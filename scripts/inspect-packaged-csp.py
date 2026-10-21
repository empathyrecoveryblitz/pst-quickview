#!/usr/bin/env python3
"""Bounded observations for a failed packaged CSP gate; never changes its rules."""

import hashlib
import importlib.util
import json
import re
import subprocess
import sys
import tempfile
from bisect import bisect_right
from collections import Counter
from pathlib import Path

sys.dont_write_bytecode = True

MAX_SOURCE_BYTES = 2 * 1024 * 1024
MAX_BINARY_BYTES = 1024 * 1024 * 1024
MAX_SLICE_BYTES = 512 * 1024 * 1024
MAX_OCCURRENCES = 100_000
MAX_LOCATIONS = 8
MAX_SAMPLES = 16
MAX_REPORT_BYTES = 64 * 1024
MAX_SOURCE_FILES = 128
MAX_ADJACENCY_SOURCE_BYTES = 8 * 1024 * 1024
MAX_ADJACENCY_CANDIDATES = 4096
MAX_ADJACENCY_BYTES = 256
MAX_ADJACENCY_MATCHES = 4
ADJACENCY_CONTEXT_BYTES = 32
ARCHITECTURES = ("x86_64", "arm64")


class DiagnosticError(RuntimeError):
    """Contains a fixed category, never input text or a file path."""


def fingerprint(value):
    return hashlib.sha256(value).hexdigest()


def _checker():
    path = Path(__file__).with_name("verify-packaged-csp.py")
    spec = importlib.util.spec_from_file_location("reviewed_packaged_csp", path)
    if spec is None or spec.loader is None:
        raise DiagnosticError("checker_unavailable")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


try:
    CHECKER = _checker()
except Exception:
    CHECKER = None


def _read_source(path):
    if path.is_symlink() or not path.is_file():
        raise DiagnosticError("source_unavailable")
    if path.stat().st_size > MAX_SOURCE_BYTES:
        raise DiagnosticError("source_size_limit")
    with path.open("rb") as handle:
        value = handle.read(MAX_SOURCE_BYTES + 1)
    if len(value) > MAX_SOURCE_BYTES:
        raise DiagnosticError("source_size_limit")
    return value


def _rust_string(literal):
    try:
        value = json.loads(literal)
    except (ValueError, TypeError):
        raise DiagnosticError("source_literal_unavailable") from None
    if not isinstance(value, str) or not value or len(value) > 8192:
        raise DiagnosticError("source_literal_unavailable")
    if any(ord(character) < 32 or ord(character) > 126 for character in value):
        raise DiagnosticError("source_literal_unavailable")
    return value.encode("ascii")


def source_literals(config_bytes, support_bytes):
    """Extract exact source facts; these labels do not approve any policy."""
    if CHECKER is None:
        raise DiagnosticError("checker_unavailable")
    if len(config_bytes) > MAX_SOURCE_BYTES or len(support_bytes) > MAX_SOURCE_BYTES:
        raise DiagnosticError("source_size_limit")
    try:
        config = json.loads(config_bytes)
        support = support_bytes.decode("utf-8")
        production = config["app"]["security"]["csp"]
    except (ValueError, UnicodeError, KeyError, TypeError):
        raise DiagnosticError("source_literal_unavailable") from None
    if not isinstance(production, str) or not production or len(production) > 8192:
        raise DiagnosticError("source_literal_unavailable")
    start = support.find("fn support_csp(")
    if start < 0:
        raise DiagnosticError("source_literal_unavailable")
    # The reviewed queue implementation delegates to a separately testable
    # origin guard. Observe that fixed span, not just the small caller wrapper.
    end = support.find("\nfn take_session(", start)
    if end < 0:
        raise DiagnosticError("source_literal_unavailable")
    function = support[start:end]
    string = r'"(?:[^"\\]|\\.)*"'
    templates = re.findall(r"directives\.push\(\s*format!\(\s*(" + string + r")\s*,?\s*\)\s*\)", function)
    tokens = re.findall(r"Some\(\s*(" + string + r")\s*\)", function)
    if len(templates) != 1 or len(tokens) != 1:
        raise DiagnosticError("source_literal_unavailable")
    template = _rust_string(templates[0])
    token = _rust_string(tokens[0])
    if not template.endswith(b"{origin}") or template.count(b"{origin}") != 1 or token != b"connect-src":
        raise DiagnosticError("source_literal_unavailable")
    template = template[: -len(b"{origin}")]
    if b"{" in template or b"}" in template or not template.startswith(token + b" "):
        raise DiagnosticError("source_literal_unavailable")
    resource_start = support.find(".on_web_resource_request(move |request, response| {")
    if resource_start < 0:
        raise DiagnosticError("source_literal_unavailable")
    resource_tail = support[resource_start:]
    resource_end = re.search(r"^[ \t]*\}\)[ \t]*\r?\n[ \t]*\.build\(\);[ \t]*\r?$", resource_tail, re.MULTILINE)
    if resource_end is None:
        raise DiagnosticError("source_literal_unavailable")
    fallbacks = re.findall(r"\.unwrap_or\(\s*(" + string + r")\s*,?\s*\)", resource_tail[: resource_end.start()])
    if len(fallbacks) != 1:
        raise DiagnosticError("source_literal_unavailable")
    return {
        "production": production.encode("utf-8"),
        "printable": CHECKER.PRINTABLE_CSP,
        "support_fallback": _rust_string(fallbacks[0]),
        "support_connect_template": template,
        "support_directive_token": token,
    }


def _ordinary_rust_strings(source):
    """Conservative lexer: comments, raw/byte strings, and character literals are not candidates."""
    cursor = 0
    ordinary = re.compile(r'"(?:[^"\\]|\\[\s\S])*"')
    character = re.compile(r"'(?:[^'\\\r\n]|\\(?:[^\r\n]|u\{[0-9a-fA-F_]+\}))'")
    while cursor < len(source):
        if source.startswith("//", cursor):
            newline = source.find("\n", cursor)
            cursor = len(source) if newline < 0 else newline + 1
        elif source.startswith("/*", cursor):
            depth, cursor = 1, cursor + 2
            while cursor < len(source) and depth:
                if source.startswith("/*", cursor):
                    depth, cursor = depth + 1, cursor + 2
                elif source.startswith("*/", cursor):
                    depth, cursor = depth - 1, cursor + 2
                else:
                    cursor += 1
            if depth:
                raise DiagnosticError("source_adjacency_unavailable")
        else:
            raw = re.match(r'(?:br|r)(#*)"', source[cursor:cursor + 260])
            if raw and (cursor == 0 or not (source[cursor - 1].isalnum() or source[cursor - 1] == "_")):
                end = source.find('"' + raw.group(1), cursor + raw.end())
                if end < 0:
                    raise DiagnosticError("source_adjacency_unavailable")
                cursor = end + 1 + len(raw.group(1))
                continue
            char = character.match(source, cursor) if source[cursor] == "'" else None
            if char:
                cursor = char.end()
                continue
            if source[cursor] != '"':
                cursor += 1
                continue
            literal = ordinary.match(source, cursor)
            if literal is None:
                raise DiagnosticError("source_adjacency_unavailable")
            start, cursor = cursor, literal.end()
            if start and (source[start - 1].isalnum() or source[start - 1] == "_"):
                continue
            try:
                yield _rust_string(literal.group())
            except DiagnosticError:
                # Unsupported escapes/non-ASCII are never guessed or classified.
                continue


def source_adjacency_catalog(sources, literals):
    """Fingerprint bounded own-source candidates; matches are observations, never approvals."""
    if not sources or len(sources) > MAX_SOURCE_FILES:
        raise DiagnosticError("source_adjacency_limit")
    total, digest, candidates = 0, hashlib.sha256(), {}

    def add(value, kind):
        if not 2 <= len(value) <= MAX_ADJACENCY_BYTES:
            return
        candidates.setdefault(value, set()).add(kind)
        if len(candidates) > MAX_ADJACENCY_CANDIDATES:
            raise DiagnosticError("source_adjacency_limit")

    for identity, data in sorted(sources):
        total += len(data)
        if len(data) > MAX_SOURCE_BYTES or total > MAX_ADJACENCY_SOURCE_BYTES:
            raise DiagnosticError("source_adjacency_limit")
        try:
            source = data.decode("utf-8")
        except UnicodeError:
            raise DiagnosticError("source_adjacency_unavailable") from None
        name = identity.encode("utf-8")
        digest.update(len(name).to_bytes(4, "big") + name + len(data).to_bytes(8, "big") + data)
        # Conventional final test modules/files are excluded; ordinary cfg(test)
        # attributes earlier in a module must not hide later production code.
        if identity.endswith("_tests.rs"):
            continue
        test_module = re.search(r"\n#\[cfg\(test\)\]\s*\nmod tests\s*\{", source)
        if test_module is not None:
            source = source[:test_module.start()]
        for value in _ordinary_rust_strings(source):
            add(value, "rust_string")
    add(literals["support_connect_template"], "reviewed_format_fragment")
    add(b"src/support_session.rs", "fixed_module_identifier")
    return {"sourceSha256": digest.hexdigest(), "sourceFileCount": len(sources),
            "sourceByteLength": total, "candidateCount": len(candidates),
            "candidates": sorted(((value, sorted(kinds)) for value, kinds in candidates.items()),
                                 key=lambda item: (-len(item[0]), fingerprint(item[0])))}


def _adjacency(binary, position, before, catalog):
    # This only proves matching adjacent bytes, not allocation ownership or safety.
    available = min(position if before else len(binary) - position, MAX_ADJACENCY_BYTES)
    context = binary[position - available:position] if before else binary[position:position + available]
    short_context = context[-ADJACENCY_CONTEXT_BYTES:] if before else context[:ADJACENCY_CONTEXT_BYTES]
    matches = []
    total = 0
    for value, kinds in catalog["candidates"]:
        if (context.endswith(value) if before else context.startswith(value)):
            total += 1
            if len(matches) < MAX_ADJACENCY_MATCHES:
                matches.append({"sha256": fingerprint(value), "byteLength": len(value), "kinds": kinds})
    return {"classification": "source_literal_match" if matches else "unclassified",
            "contextByteLength": len(short_context), "contextSha256": fingerprint(short_context),
            "matches": matches, "matchesTruncated": total > MAX_ADJACENCY_MATCHES}


def _own_crate_sources(support_path, support_bytes):
    sources, total = [], 0
    for path in support_path.parent.iterdir():
        if path.suffix != ".rs" or path.is_symlink() or not path.is_file():
            continue
        if len(sources) >= MAX_SOURCE_FILES:
            raise DiagnosticError("source_adjacency_limit")
        data = _read_source(path)
        total += len(data)
        if total > MAX_ADJACENCY_SOURCE_BYTES:
            raise DiagnosticError("source_adjacency_limit")
        if path.name == support_path.name and data != support_bytes:
            raise DiagnosticError("source_changed")
        sources.append((path.name, data))
    if not any(name == support_path.name for name, _ in sources):
        raise DiagnosticError("source_adjacency_unavailable")
    return sorted(sources)


def _offsets(binary, literal):
    offsets = []
    cursor = 0
    while True:
        cursor = binary.find(literal, cursor)
        if cursor < 0:
            return offsets
        if len(offsets) >= MAX_OCCURRENCES:
            raise DiagnosticError("observation_limit_exceeded")
        offsets.append(cursor)
        cursor += 1


def _boundary(binary, position):
    if position < 0:
        return "start"
    if position >= len(binary):
        return "end"
    byte = binary[position]
    if byte == 0:
        return "nul"
    if byte in b"\"'":
        return "quote"
    if byte in b" \t\n\f\r":
        return "whitespace"
    if byte in b";,":
        return "separator"
    return "printable" if 32 <= byte <= 126 else "binary"


def _prefix(binary, start, architecture):
    for prefix in CHECKER.REVIEWED_PACKED_PREFIXES.get(architecture, ()):
        if binary.endswith(prefix, 0, start):
            return {b"PST QuickView": "product_name", b"index.html": "main_document"}.get(prefix, "other_reviewed_prefix")
    return "none"


def _failure_category(error):
    message = str(error)
    for fragment, category in (
        ("exactly one of each", "policy_occurrence"),
        ("prefixes a reviewed CSP", "policy_prefix"),
        ("extends a reviewed CSP", "policy_extension"),
        ("unexpected CSP header inventory", "header_inventory"),
        ("unreviewed CSP directive", "unreviewed_directive"),
        ("appends an unreviewed", "policy_append"),
    ):
        if fragment in message:
            return category
    return "verifier_failure"


def inspect_slice(binary, architecture, literals, adjacency_catalog):
    if architecture not in ARCHITECTURES or len(binary) > MAX_SLICE_BYTES:
        raise DiagnosticError("slice_limit_or_architecture")
    try:
        CHECKER.verify_slice(binary, architecture, (literals["production"], literals["printable"]))
        existing_gate = {"passed": True, "failureCategory": "none"}
    except CHECKER.VerificationError as error:
        existing_gate = {"passed": False, "failureCategory": _failure_category(error)}
    ranges = []
    observations = {}
    for label, literal in literals.items():
        locations = _offsets(binary, literal)
        if label != "support_directive_token":
            ranges.extend((start, start + len(literal)) for start in locations)
        observations[label] = {
            "sha256": fingerprint(literal),
            "byteLength": len(literal),
            "occurrences": len(locations),
            "locationsTruncated": len(locations) > MAX_LOCATIONS,
            "locations": [{
                "offset": start,
                "before": _boundary(binary, start - 1),
                "after": _boundary(binary, start + len(literal)),
                "recognizedPrefix": _prefix(binary, start, architecture),
                **({"adjacency": {"before": _adjacency(binary, start, True, adjacency_catalog),
                                  "after": _adjacency(binary, start + len(literal), False, adjacency_catalog)}}
                   if label in ("support_fallback", "support_connect_template") else {}),
            } for start in locations[:MAX_LOCATIONS]],
        }
    spellings = Counter()
    for index, match in enumerate(CHECKER.CSP_HEADER_PATTERN.finditer(binary)):
        if index >= MAX_OCCURRENCES:
            raise DiagnosticError("observation_limit_exceeded")
        spellings[match.group()] += 1
    other_spellings = {fingerprint(spelling): count for spelling, count in spellings.items()
                       if spelling not in (b"Content-Security-Policy", b"content-security-policy")}
    histogram = Counter()
    samples = []
    directives = 0
    merged = []
    for start, end in sorted(ranges):
        if merged and start <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(end, merged[-1][1]))
        else:
            merged.append((start, end))
    range_starts = [start for start, _ in merged]
    for match in CHECKER.CSP_DIRECTIVE_PATTERN.finditer(binary):
        directives += 1
        if directives > MAX_OCCURRENCES:
            raise DiagnosticError("observation_limit_exceeded")
        containing_range = bisect_right(range_starts, match.start()) - 1
        if containing_range >= 0 and match.start() < merged[containing_range][1]:
            continue
        # The pattern is the verifier's fixed directive enum, never arbitrary text.
        category = match.group().decode("ascii").lower()
        histogram[category] += 1
        if len(samples) < MAX_SAMPLES:
            samples.append({
                "directive": category, "offset": match.start(),
                "before": _boundary(binary, match.start() - 1),
                "after": _boundary(binary, match.end()),
                "contextSha256": fingerprint(binary[max(0, match.start() - 64) : match.end() + 64]),
            })
    return {
        "architecture": architecture, "sha256": fingerprint(binary), "byteLength": len(binary),
        "existingVerifier": existing_gate,
        "adjacencySourceCatalog": {key: value for key, value in adjacency_catalog.items() if key != "candidates"},
        "headerSpellings": {"canonical": spellings[b"Content-Security-Policy"],
                            "lowercase": spellings[b"content-security-policy"],
                            "otherCase": sum(other_spellings.values()),
                            "otherCaseFingerprints": other_spellings},
        "sourceLiteralObservations": observations,
        "outsideKnownLiteralDirectiveHistogram": dict(sorted(histogram.items())),
        "outsideKnownLiteralSamples": samples,
        "samplesTruncated": sum(histogram.values()) > len(samples),
    }


def _file_fingerprint(path):
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            size += len(chunk)
            if size > MAX_BINARY_BYTES:
                raise DiagnosticError("binary_size_limit")
            digest.update(chunk)
    return digest.hexdigest()


def inspect_app(config_path, support_path, binary_path):
    config_bytes, support_bytes = _read_source(config_path), _read_source(support_path)
    literals = source_literals(config_bytes, support_bytes)
    adjacency_catalog = source_adjacency_catalog(_own_crate_sources(support_path, support_bytes), literals)
    if binary_path.is_symlink() or not binary_path.is_file():
        raise DiagnosticError("binary_unavailable")
    if binary_path.stat().st_size > MAX_BINARY_BYTES:
        raise DiagnosticError("binary_size_limit")
    report = {"schema": 1, "diagnosticOnly": True,
              "sourceFingerprints": {"config": fingerprint(config_bytes), "support": fingerprint(support_bytes)},
              "binarySha256": _file_fingerprint(binary_path), "slices": []}
    with tempfile.TemporaryDirectory(prefix="pst-quickview-csp-inspection-") as temporary:
        for architecture in ARCHITECTURES:
            output = Path(temporary) / architecture
            try:
                result = subprocess.run(["lipo", str(binary_path.resolve()), "-thin", architecture, "-output", str(output)],
                                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False, timeout=30)
            except FileNotFoundError:
                raise DiagnosticError("architecture_tool_unavailable") from None
            except subprocess.TimeoutExpired:
                raise DiagnosticError("architecture_extraction_timeout") from None
            if result.returncode != 0 or not output.is_file():
                raise DiagnosticError("architecture_extraction_failed")
            if output.stat().st_size > MAX_SLICE_BYTES:
                raise DiagnosticError("slice_limit_or_architecture")
            report["slices"].append(inspect_slice(output.read_bytes(), architecture, literals, adjacency_catalog))
    return report


def main(argv):
    if len(argv) != 4:
        report, status = {"schema": 1, "diagnosticOnly": True, "errorCategory": "argument_count"}, 2
    else:
        try:
            report = inspect_app(*(Path(argument) for argument in argv[1:]))
            # Observations never turn an existing verifier failure into success.
            status = 0 if all(item["existingVerifier"]["passed"] for item in report["slices"]) else 1
        except DiagnosticError as error:
            report, status = {"schema": 1, "diagnosticOnly": True, "errorCategory": error.args[0]}, 2
        except Exception:
            report, status = {"schema": 1, "diagnosticOnly": True, "errorCategory": "diagnostic_unavailable"}, 2
    encoded = json.dumps(report, sort_keys=True, separators=(",", ":"))
    if len(encoded.encode("utf-8")) > MAX_REPORT_BYTES:
        encoded, status = '{"diagnosticOnly":true,"errorCategory":"report_limit_exceeded","schema":1}', 2
    print(encoded)
    return status


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
