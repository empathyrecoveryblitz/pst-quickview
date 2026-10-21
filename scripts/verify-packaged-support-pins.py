#!/usr/bin/env python3
"""Read-only check that both app slices contain the current configured trust pin."""
import base64
import json
import re
import subprocess
import sys
import tempfile
import unicodedata
from pathlib import Path


class PinError(RuntimeError):
    """Fixed categories only: never echo secrets, untrusted bytes, or local paths."""


def unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    value: dict[str, object] = {}
    for key, item in pairs:
        if key in value:
            raise ValueError()
        value[key] = item
    return value


def reviewed_pin(source: bytes) -> bytes:
    if not source or len(source) > 2048:
        raise PinError("support_pin_unavailable")
    try:
        value = json.loads(source.decode("utf-8"), object_pairs_hook=unique_object)
        if not isinstance(value, dict) or set(value) != {"v", "origin", "operatorPublicKey", "operatorLabel"} or type(value["v"]) is not int or value["v"] != 3:
            raise ValueError()
        label = r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?"
        if not isinstance(value["origin"], str) or not re.fullmatch(r"https://" + label + r"\." + label + r"\.workers\.dev", value["origin"]):
            raise ValueError()
        key = value["operatorPublicKey"]
        if not isinstance(key, str) or not re.fullmatch(r"[A-Za-z0-9_-]{87}", key):
            raise ValueError()
        decoded = base64.urlsafe_b64decode(key + "=")
        if len(decoded) != 65 or decoded[0] != 4 or base64.urlsafe_b64encode(decoded).decode().rstrip("=") != key:
            raise ValueError()
        name = value["operatorLabel"]
        if not isinstance(name, str) or not 1 <= len(name.encode("utf-16-le")) // 2 <= 80 or name.strip() != name or any(unicodedata.category(c) == "Cc" for c in name):
            raise ValueError()
    except (ValueError, TypeError, KeyError):
        raise PinError("support_pin_unavailable") from None
    return source


def rust_tokens(source: str) -> list[str]:
    """Keep literal tokens intact and ignore comments, including nested blocks."""
    tokens: list[str] = []
    position = 0
    while position < len(source):
        if source[position].isspace():
            position += 1
        elif source.startswith("//", position):
            end = source.find("\n", position + 2)
            position = len(source) if end < 0 else end + 1
        elif source.startswith("/*", position):
            depth = 1
            position += 2
            while depth and position < len(source):
                if source.startswith("/*", position):
                    depth += 1
                    position += 2
                elif source.startswith("*/", position):
                    depth -= 1
                    position += 2
                else:
                    position += 1
            if depth:
                raise ValueError()
        else:
            raw = re.match(r'r(#+)?"', source[position:])
            if raw:
                closing = '"' + (raw.group(1) or "")
                end = source.find(closing, position + len(raw.group()))
                if end < 0:
                    raise ValueError()
                end += len(closing)
            elif source[position] == '"':
                literal = re.match(r'"(?:\\[\s\S]|[^"\\])*"', source[position:])
                if not literal:
                    raise ValueError()
                end = position + len(literal.group())
            else:
                token = re.match(r"'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\])'|[A-Za-z_][A-Za-z_0-9]*|->|::|[^\s]", source[position:])
                if not token:
                    raise ValueError()
                end = position + len(token.group())
            tokens.append(source[position:end])
            position = end
    return tokens


def verify_native_binding(source: bytes) -> None:
    try:
        if not source or len(source) > 1024 * 1024:
            raise ValueError()
        tokens = rust_tokens(source.decode("utf-8"))
        binding = rust_tokens('const SERVICE_CONFIG: &str = include_str!("../../public/support/service.json");')
        parser = rust_tokens('fn service_config() -> SupportResult<ServiceConfig> { parse_service_config(SERVICE_CONFIG) }')
        for required in (binding, parser):
            if sum(tokens[index:index + len(required)] == required for index in range(len(tokens))) != 1:
                raise ValueError()
        if tokens.count("SERVICE_CONFIG") != 2:
            raise ValueError()
    except (ValueError, TypeError):
        raise PinError("support_source_binding_mismatch") from None


def verify_slice(binary: bytes, source: bytes) -> None:
    pin = reviewed_pin(source)
    if len(binary) > 512 * 1024 * 1024 or pin not in binary:
        raise PinError("packaged_support_pin_mismatch")


def reviewed_update_pin(source: bytes, service: bytes, tauri: bytes, native: bytes) -> bytes:
    try:
        if not source or len(source) > 8192:
            raise ValueError()
        value = json.loads(source.decode("utf-8"), object_pairs_hook=unique_object)
        support = json.loads(reviewed_pin(service), object_pairs_hook=unique_object)
        configuration = json.loads(tauri, object_pairs_hook=unique_object)
        if set(value) != {"v", "origin", "manifestPath", "artifactPrefix", "publicKey", "channel"} or type(value["v"]) is not int or value["v"] != 1:
            raise ValueError()
        if value["origin"] != support["origin"] or value["manifestPath"] != "/updates/development.json" or value["artifactPrefix"] != "/updates/artifacts/" or value["channel"] != "support-development":
            raise ValueError()
        key = value["publicKey"]
        if not isinstance(key, str) or len(key) > 8192:
            raise ValueError()
        decoded = base64.b64decode(key, validate=True)
        lines = decoded.splitlines()
        if base64.b64encode(decoded).decode() != key or len(lines) != 2 or not lines[0].startswith(b"untrusted comment: minisign public key: "):
            raise ValueError()
        raw_key = base64.b64decode(lines[1], validate=True)
        if len(raw_key) != 42 or raw_key[:2] != b"Ed":
            raise ValueError()
        expected = {"pubkey": key, "endpoints": [value["origin"] + value["manifestPath"]], "requireSignedVersion": True, "allowDowngrades": False}
        if configuration.get("plugins", {}).get("updater") != expected:
            raise ValueError()
        tokens = rust_tokens(native.decode("utf-8"))
        for required in (
            rust_tokens('const CONFIG: &str = include_str!("../../public/support/update-service.json");'),
            rust_tokens('let config = config(CONFIG, &identity.0)?;'),
            rust_tokens('update.verify(&bytes).map_err(|_| fail())?;'),
        ):
            if sum(tokens[index:index + len(required)] == required for index in range(len(tokens))) != 1:
                raise ValueError()
    except (ValueError, TypeError, KeyError, AttributeError):
        raise PinError("update_pin_unavailable") from None
    return source


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        print("usage: verify-packaged-support-pins.py SERVICE_CONFIG APP_BINARY", file=sys.stderr)
        return 2
    try:
        config, binary = Path(argv[1]), Path(argv[2])
        if config.is_symlink() or not config.is_file() or config.stat().st_size > 2048 or binary.is_symlink() or not binary.is_file():
            raise PinError("support_pin_unavailable")
        root = Path(__file__).resolve().parents[1]
        native = root / "src-tauri/src/support_session.rs"
        if config.resolve() != root / "public/support/service.json" or native.is_symlink() or not native.is_file() or native.stat().st_size > 1024 * 1024:
            raise PinError("support_source_binding_mismatch")
        verify_native_binding(native.read_bytes())
        source = reviewed_pin(config.read_bytes())
        update_source = reviewed_update_pin((root / "public/support/update-service.json").read_bytes(), source,
                                           (root / "src-tauri/tauri.conf.json").read_bytes(), (root / "src-tauri/src/updater.rs").read_bytes())
        with tempfile.TemporaryDirectory(prefix="pqv-support-pin-") as temporary:
            for arch in ("x86_64", "arm64"):
                target = Path(temporary) / arch
                result = subprocess.run(["lipo", str(binary), "-thin", arch, "-output", str(target)], capture_output=True, check=False, timeout=30)
                if result.returncode != 0 or target.stat().st_size > 512 * 1024 * 1024:
                    raise PinError("support_slice_unavailable")
                contents = target.read_bytes()
                verify_slice(contents, source)
                if update_source not in contents:
                    raise PinError("packaged_update_pin_mismatch")
        print("PASS: both packaged architectures contain the exact current support and signed-update trust pins")
        return 0
    except (OSError, subprocess.SubprocessError, PinError) as error:
        print(str(error) if isinstance(error, PinError) else "support_pin_check_unavailable", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
