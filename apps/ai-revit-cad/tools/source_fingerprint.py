"""Hash public application source and the exact separately fetched SDK revision.

No local credentials or absolute paths enter the manifest. This is a source
identity helper, not compiler input tracing or engineering certification.
"""
from __future__ import annotations
import argparse, hashlib, json, os, subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SDK_REVISION = "105158e08c89d47f57af4621420ed68eb3a147ce"
EXCLUDED = {".git", "target", "artifacts", "reports", ".deps", "__pycache__", "verification", "runtime"}

def fingerprint():
    sdk = ROOT.parents[1] / ".deps" / "makepad"
    revision = subprocess.check_output(["git", "-C", str(sdk), "rev-parse", "HEAD"], text=True).strip()
    if revision != SDK_REVISION:
        raise RuntimeError("SDK differs from the pinned public revision; review compatibility first")
    dirty = subprocess.check_output(["git", "-C", str(sdk), "status", "--porcelain", "--untracked-files=no"], text=True).strip()
    if dirty:
        raise RuntimeError("SDK contains tracked changes; refusing a clean public-source identity")
    rows = []
    for folder, dirs, names in os.walk(ROOT, followlinks=False):
        dirs[:] = [name for name in dirs if name not in EXCLUDED and not (Path(folder)/name).is_symlink()]
        for name in names:
            path = Path(folder)/name
            if path.is_symlink():
                continue
            if path.suffix not in {".rs", ".toml", ".ptx", ".py"} and name not in {"Cargo.lock", "LICENSE", "LICENSE-MIT", "LICENSE-APACHE", "NOTICE"}:
                continue
            raw = path.read_bytes()
            rows.append({"path":path.relative_to(ROOT).as_posix(), "bytes":len(raw), "sha256":hashlib.sha256(raw).hexdigest()})
    rows.sort(key=lambda row: row["path"])
    payload = {"schema":"ai-revit-cad-public-source-v1", "scope":"Application Rust/manifests/PTX/helper source, license notices and lock; exact clean SDK Git revision. No compiler/OS SDK tracing.", "makepad_revision":revision, "files":rows}
    raw = (json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=False)+"\n").encode("utf-8")
    return hashlib.sha256(raw).hexdigest(), raw

if __name__ == "__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, help="optional new local manifest file; not for committing")
    args=parser.parse_args()
    digest, raw=fingerprint()
    if args.manifest:
        with args.manifest.open("xb") as output:
            output.write(raw)
    print(digest)
