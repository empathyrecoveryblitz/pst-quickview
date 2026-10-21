#!/usr/bin/env python3
"""Verify the exact CSP inventory embedded in both macOS executable slices."""

import json
import re
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path


class VerificationError(RuntimeError):
    """A packaged CSP invariant failed."""


PRINTABLE_CSP = (
    b"default-src 'none'; img-src data:; style-src 'unsafe-inline'; "
    b"object-src 'none'; base-uri 'none'; form-action 'none'; "
    b"frame-src 'none'; connect-src 'none'"
)
# Fixed, reviewed support literals, not policies learned from arbitrary source.
SUPPORT_FALLBACK_CSP = (
    b"default-src 'none'; script-src 'self'; style-src 'self'; "
    b"frame-src 'none'; frame-ancestors 'none'; object-src 'none'; "
    b"media-src 'none'; base-uri 'none'; form-action 'none'"
)
# This is a format-string fragment, NOT a complete active policy. The guarded
# native constructor appends one validated session origin at runtime.
SUPPORT_CONNECT_PREFIX = b"connect-src ipc: http://ipc.localhost "
SUPPORT_FALLBACK_BEFORE = b"Content-Security-Policy"
SUPPORT_FALLBACK_AFTER = b"Cache-Control"
# Exact observed linker layouts: the arm64 fragment was previously coalesced
# after the source literal "tauri" and is now also emitted after a NUL boundary.
# No arbitrary printable prefix is accepted.
SUPPORT_CONNECT_BEFORE = {"x86_64": (b"\x00",), "arm64": (b"tauri", b"\x00")}
SUPPORT_CONNECT_AFTER = b"; "
CONNECT_TOKEN_PATTERN = re.compile(rb"connect-src", re.IGNORECASE)
CSP_HEADER = b"Content-Security-Policy"
EXPECTED_CSP_HEADER_INVENTORY = Counter(
    {
        b"Content-Security-Policy": 3,
        b"content-security-policy": 4,
    }
)
CSP_HEADER_PATTERN = re.compile(re.escape(CSP_HEADER), re.IGNORECASE)
# Rust may coalesce adjacent immutable strings without a NUL byte. These are the
# reviewed source literals immediately before the production CSP in the two
# architecture slices. The x86_64 prefix remains the fixed application UI name
# even when an external flavor changes the bundle product name; Info.plist
# identity is verified separately. Any other printable prefix fails closed.
REVIEWED_PACKED_PREFIXES = {
    "x86_64": (b"PST QuickView",),
    "arm64": (b"index.html", b"PST QuickView"),
}
CSP_DIRECTIVES = (
    b"base-uri",
    b"block-all-mixed-content",
    b"child-src",
    b"connect-src",
    b"default-src",
    b"fenced-frame-src",
    b"font-src",
    b"form-action",
    b"frame-ancestors",
    b"frame-options",
    b"frame-src",
    b"img-src",
    b"manifest-src",
    b"media-src",
    b"navigate-to",
    b"object-src",
    b"plugin-types",
    b"prefetch-src",
    b"referrer",
    b"report-to",
    b"report-uri",
    b"require-sri-for",
    b"require-trusted-types-for",
    b"sandbox",
    b"script-src",
    b"script-src-attr",
    b"script-src-elem",
    b"style-src",
    b"style-src-attr",
    b"style-src-elem",
    b"trusted-types",
    b"upgrade-insecure-requests",
    b"webrtc",
    b"worker-src",
)
CSP_DIRECTIVE_PATTERN = re.compile(
    rb"(?<![A-Za-z0-9-])(?:"
    + b"|".join(re.escape(name) for name in sorted(CSP_DIRECTIVES, key=len, reverse=True))
    + rb")(?=[\t\n\f\r ;,\"'\x00]|$)",
    re.IGNORECASE,
)
APPENDED_DIRECTIVE_PATTERN = re.compile(
    rb"\s*;\s*[a-z][a-z0-9-]*(?=[\t\n\f\r ;,\"'\x00]|$)",
    re.IGNORECASE,
)
APP_ARCHITECTURES = ("x86_64", "arm64")


def reviewed_policies(tauri_config_path: Path) -> tuple[bytes, bytes]:
    with tauri_config_path.open(encoding="utf-8") as handle:
        csp = json.load(handle).get("app", {}).get("security", {}).get("csp")
    if not isinstance(csp, str):
        raise VerificationError("reviewed production CSP is missing")
    return csp.encode(), PRINTABLE_CSP


def verify_slice(
    binary: bytes,
    arch: str,
    policies: tuple[bytes, bytes],
) -> None:
    if arch not in APP_ARCHITECTURES:
        raise VerificationError("packaged executable has an unreviewed architecture")
    policy_ranges: list[tuple[int, int]] = []
    packed_prefixes = REVIEWED_PACKED_PREFIXES.get(arch, ())
    for policy_index, policy in enumerate(policies):
        if binary.count(policy) != 1:
            raise VerificationError(
                f"packaged {arch} executable does not contain exactly one of each reviewed CSP"
            )
        start = binary.index(policy)
        end = start + len(policy)
        has_leading_boundary = (
            start == 0
            or binary[start - 1] in b'\x00"'
            or (
                policy_index == 0
                and any(binary[:start].endswith(prefix) for prefix in packed_prefixes)
            )
        )
        if not has_leading_boundary:
            raise VerificationError(
                f"packaged {arch} executable prefixes a reviewed CSP"
            )
        if end < len(binary) and binary[end] in b" \t\n\f\r;,":
            raise VerificationError(
                f"packaged {arch} executable extends a reviewed CSP"
            )
        policy_ranges.append((start, end))

    inventory_ranges = list(policy_ranges)
    for literal, before, after in (
        (SUPPORT_FALLBACK_CSP, (SUPPORT_FALLBACK_BEFORE,), SUPPORT_FALLBACK_AFTER),
        (SUPPORT_CONNECT_PREFIX, SUPPORT_CONNECT_BEFORE[arch], SUPPORT_CONNECT_AFTER),
    ):
        if binary.count(literal) != 1:
            raise VerificationError(
                f"packaged {arch} executable does not contain exactly one of each reviewed support literal"
            )
        start = binary.index(literal)
        end = start + len(literal)
        if not any(binary.endswith(prefix, 0, start) for prefix in before) or not binary.startswith(after, end):
            raise VerificationError(
                f"packaged {arch} executable changes reviewed support literal adjacency"
            )
        inventory_ranges.append((start, end))

    header_inventory = Counter(
        match.group() for match in CSP_HEADER_PATTERN.finditer(binary)
    )
    if header_inventory != EXPECTED_CSP_HEADER_INVENTORY:
        raise VerificationError(
            f"packaged {arch} executable contains an unexpected CSP header inventory"
        )

    connect_tokens = list(CONNECT_TOKEN_PATTERN.finditer(binary))
    if len(connect_tokens) != 3 or any(
        not any(start <= match.start() and match.end() <= end for start, end in inventory_ranges)
        for match in connect_tokens
    ):
        raise VerificationError(
            f"packaged {arch} executable contains an unreviewed connect-src token inventory"
        )

    for match in CSP_DIRECTIVE_PATTERN.finditer(binary):
        if not any(start <= match.start() and match.end() <= end for start, end in inventory_ranges):
            raise VerificationError(
                f"packaged {arch} executable contains an unreviewed CSP directive: "
                f"{match.group().decode()}"
            )

    for _, end in policy_ranges:
        if APPENDED_DIRECTIVE_PATTERN.match(binary[end : end + 256]):
            raise VerificationError(
                f"packaged {arch} executable appends an unreviewed CSP directive"
            )


def verify_app(tauri_config_path: Path, app_binary_path: Path) -> None:
    policies = reviewed_policies(tauri_config_path)
    if not app_binary_path.is_file():
        raise VerificationError(f"packaged executable is missing: {app_binary_path}")

    with tempfile.TemporaryDirectory(prefix="pst-quickview-csp-") as temporary_dir:
        for arch in APP_ARCHITECTURES:
            slice_path = Path(temporary_dir) / arch
            result = subprocess.run(
                ["lipo", str(app_binary_path), "-thin", arch, "-output", str(slice_path)],
                capture_output=True,
                text=True,
                check=False,
            )
            if result.returncode != 0:
                raise VerificationError(
                    f"could not extract packaged {arch} executable slice: "
                    f"{result.stderr.strip()}"
                )
            verify_slice(slice_path.read_bytes(), arch, policies)


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        print(
            "usage: verify-packaged-csp.py TAURI_CONFIG APP_BINARY",
            file=sys.stderr,
        )
        return 2
    try:
        verify_app(Path(argv[1]), Path(argv[2]))
    except (OSError, json.JSONDecodeError, VerificationError) as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
