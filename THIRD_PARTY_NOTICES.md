# Third-Party Notices

Deskctl can package the following third-party artifacts:

| Artifact | Source | License |
|---|---|---|
| PaddleOCR PP-OCRv4 detection/recognition models | RapidOCR / PaddleOCR | Apache-2.0 |
| PaddleOCR character dictionary | PaddleOCR | Apache-2.0 |
| OmniParser icon detection ONNX export | onnx-community / OmniParser | MIT |
| Microsoft ONNX Runtime | Microsoft | MIT |
| Nuphus desktop implementation references | Nuphus | Apache-2.0 |

Note on the icon detector (AC-PKG-006): the bundled `models/icon_detect.onnx` is the
`onnx-community/OmniParser-icon_detect_640x640` ONNX export (published under MIT); its detection
backbone derives from YOLO, but no Ultralytics code, weights or training artifacts are
redistributed. Upstream PaddleOCR models are Apache-2.0; ONNX Runtime is MIT.

Exact hashes are recorded in `models/hashes.json` and
`third_party/onnxruntime/win-x64/hashes.json`, and verified by
`scripts/verify-models.ps1`.

These notices must be included in every installer and portable release.

