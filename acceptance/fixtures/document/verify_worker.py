"""Exercise the packaged document worker against deterministic local fixtures.

Usage: python verify_worker.py PATH_TO_pab-document-worker[.exe]
The worker must have its sibling document-runtime directory installed.
"""
import json
from difflib import SequenceMatcher
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent


def extract(worker: Path, filename: str) -> dict:
    response = subprocess.run(
        [str(worker)],
        input=json.dumps({"path": str(ROOT / filename), "ocr_mode": "auto"}).encode(),
        capture_output=True,
        timeout=30,
        check=True,
    )
    return json.loads(response.stdout)


def main(worker: Path) -> None:
    native = extract(worker, "native-table.pdf")["document"]
    assert len(native["pages"]) == 1
    assert "payment due within 30 days" in native["pages"][0]["text"]

    scan = extract(worker, "scan-zh-en.pdf")["document"]
    assert SequenceMatcher(None, "中文扫描合同", scan["pages"][0]["text"].splitlines()[0][:6]).ratio() >= 0.8
    assert "Invoice total 12345 USD" in scan["pages"][0]["text"]
    assert scan["pages"][0]["ocr_confidence"] is not None

    mixed = extract(worker, "mixed.pdf")["document"]
    assert len(mixed["pages"]) == 2
    assert "Pixels Risk Clause 7" in mixed["pages"][0]["text"]
    assert "中文" in mixed["pages"][1]["text"] and "合同" in mixed["pages"][1]["text"]

    docx = extract(worker, "heading-table.docx")["document"]
    assert "风险评估" in docx["text"]
    assert docx["tables"][0]["cells"][1] == ["设备", "42"]

    xlsx = extract(worker, "sheets-formula.xlsx")["document"]
    assert [sheet["name"] for sheet in xlsx["sheets"]] == ["Revenue", "风险"]
    cells = {cell["address"]: cell for cell in xlsx["sheets"][0]["cells"]}
    assert cells["B2"]["value"] == "42"
    assert cells["C2"]["formula"] == "B2*2"

    pptx = extract(worker, "slides-notes.pptx")["document"]
    assert len(pptx["pages"]) == 2
    assert "讲者备注" in pptx["pages"][0]["speaker_notes"]

    image = extract(worker, "scan-zh-en.png")["document"]
    assert "Invoice total 12345 USD" in image["text"]

    for filename, code in (
        ("encrypted.pdf", "password_required"),
        ("malformed.pdf", "worker_failed"),
        ("oversized-expanded.docx", "resource_limit"),
    ):
        assert extract(worker, filename)["error"].startswith(code), filename
    print("10 document worker fixtures passed")


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
