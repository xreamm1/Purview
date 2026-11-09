#!/usr/bin/env python3
"""Write signed Purview release notes; GitHub assets can remain EXE-only.

Requires OpenSSL with Ed25519 support. Private material stays in a temporary
folder beside the private key (outside git); only public signatures enter notes.
For a self-contained Windows package, use the release version as base (vVERSION)
and bundled as bootstrap-tag.
"""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile

REPO = "xreamm1/Purview"
FENCE = "```purview-update-v1\n"


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run(openssl, *args):
    completed = subprocess.run([openssl, *map(str, args)], capture_output=True)
    if completed.returncode:
        raise ValueError("OpenSSL could not sign or verify the release manifest.")
    return completed.stdout


def write_manifest(args):
    if not re.fullmatch(r"launcher@[0-9][A-Za-z0-9.+-]*", args.tag):
        raise ValueError("tag must be launcher@VERSION")
    if not all(re.fullmatch(r"[A-Za-z0-9._-]+", v) for v in (args.base, args.bootstrap_tag)):
        raise ValueError("base and bootstrap tag must be simple upstream version names")
    notes = args.notes.read_text(encoding="utf-8") if args.notes else "Purview"
    if FENCE in notes:
        raise ValueError("notes already contain an update manifest")
    trusted = [line.strip() for line in args.public_keys.read_text(encoding="utf-8").splitlines()
               if line.strip() and not line.startswith("#")]
    with tempfile.TemporaryDirectory(prefix="manifest-", dir=args.key.parent) as temporary:
        work = Path(temporary)
        private = work / "key.der"
        private.write_bytes(base64.b64decode(args.key.read_text().strip(), validate=True))
        public = work / "public.der"
        run(args.openssl, "pkey", "-inform", "DER", "-in", private,
            "-pubout", "-outform", "DER", "-out", public)
        public_key = base64.b64encode(public.read_bytes()[-32:]).decode("ascii")
        if public_key not in trusted:
            raise ValueError("private key does not match the launcher's embedded update keys")

        def signed(path, name, context):
            digest = sha256(path)
            message = work / "message"
            message.write_bytes(f"horizon release {context}\n{digest}  {name}\n".encode("utf-8"))
            signature = work / "signature"
            run(args.openssl, "pkeyutl", "-sign", "-rawin", "-inkey", private,
                "-keyform", "DER", "-in", message, "-out", signature)
            run(args.openssl, "pkeyutl", "-verify", "-rawin", "-pubin", "-inkey", public,
                "-keyform", "DER", "-in", message, "-sigfile", signature)
            return {"asset": name, "sha256": digest,
                    "signature": base64.b64encode(signature.read_bytes()).decode("ascii")}

        manifest = {"tag": args.tag, **signed(args.launcher, "purview.exe",
                    f"purview-launcher:{REPO}:{args.tag}")}
        manifest["game"] = {**signed(args.game, "purview-game.exe",
            f"purview-game:{REPO}:{args.tag}:{args.base}:{args.bootstrap_tag}"),
            "base": args.base, "bootstrap_tag": args.bootstrap_tag}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(notes.rstrip() + "\n\n" + FENCE
                        + json.dumps(manifest, indent=2) + "\n```\n", encoding="utf-8", newline="\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--launcher", type=Path, required=True)
    parser.add_argument("--game", type=Path, required=True)
    parser.add_argument("--base", required=True)
    parser.add_argument("--bootstrap-tag", required=True)
    parser.add_argument("--key", type=Path, default=Path.home() / ".local/share/purview-release-signing/release-key.b64")
    parser.add_argument("--public-keys", type=Path,
                        default=Path(__file__).resolve().parents[1] / "launcher/update-keys.txt")
    parser.add_argument("--notes", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--openssl", default="openssl")
    args = parser.parse_args()
    try:
        write_manifest(args)
    except (OSError, ValueError) as error:
        parser.exit(1, f"purview_release_manifest: {error}\n")
    print(f"Signed release notes written to {args.out}")


if __name__ == "__main__":
    main()
