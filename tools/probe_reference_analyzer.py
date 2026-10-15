"""Run only a hash-pinned reference analyzer over two synthetic CSV files."""
from __future__ import annotations
import argparse
import csv
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
import tempfile

COMMIT = "fe16728994e4599242b3fd74b2f1ebff702063a9"
LF_SHA256 = "150dc73f335e71ed343a5f9fa5ef9c1334b17c26dcc4c3aec008a83d9fbdf069"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    raw = args.source.read_bytes()
    normalized_sha = hashlib.sha256(raw.replace(b"\r\n", b"\n")).hexdigest()
    if normalized_sha != LF_SHA256:
        raise SystemExit("Source differs from the inspected commit; refusing to import.")
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location("pinned_reference_analyzer", args.source)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    cases = [
        ("keyword_false_positive", [["D:\\Projects\\temp\\invoice.db", 104857600]]),
        ("excluded_child_under_selected_parent", [
            ["C:\\Users\\demo\\AppData\\Local\\Temp\\", 1073741824],
            ["C:\\Users\\demo\\AppData\\Local\\Temp\\.ssh\\id_ed25519", 104857600],
        ]),
    ]
    results = []
    # TemporaryDirectory owns only these generated CSV fixtures, never target paths.
    with tempfile.TemporaryDirectory(prefix="cleanmanager-analyzer-probe-") as tmp:
        for name, rows in cases:
            path = Path(tmp) / (name + ".csv")
            with path.open("w", encoding="utf-8", newline="") as stream:
                writer = csv.writer(stream)
                writer.writerow(["File Name", "Size"])
                writer.writerows(rows)
            value = module.analyze_csv(str(path))
            selected = {k: v["items"] for k, v in value["categories"].items()}
            assert len(selected["high"]) == 1
            assert selected["high"][0]["path"] == rows[0][0]
            assert selected["high"][0]["safe"] is True
            assert not selected["medium"] and not selected["low"]
            results.append({"case": name, "input_rows": rows, "selected": selected})
    output = {
        "source_commit": COMMIT,
        "source_url": f"https://github.com/iziqing/-disk-cleaner/blob/{COMMIT}/analyze.py",
        "source_sha256": hashlib.sha256(raw).hexdigest(),
        "source_lf_normalized_sha256": normalized_sha,
        "scope": "Synthetic CSV classification only. No vendor scan, backup, script generation or deletion executed.",
        "cases": results,
        "assertions": "Both classification observations reproduced; this does not demonstrate an actual deletion.",
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(output, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"status": "reproduced", "cases": len(results), "output": str(args.output)}))


if __name__ == "__main__":
    main()
