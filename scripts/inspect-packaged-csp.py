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
    following = re.search(r"\n(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s", support[start + 1 :])
    if following is None:
        raise DiagnosticError("source_literal_unavailable")
    function = support[start : start + 1 + following.start()]
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


def inspect_slice(binary, architecture, literals):
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
            report["slices"].append(inspect_slice(output.read_bytes(), architecture, literals))
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
