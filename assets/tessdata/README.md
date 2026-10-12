Offline OCR language data from [tesseract-ocr/tessdata_fast](https://github.com/tesseract-ocr/tessdata_fast), commit `87416418657359cb625c412a48b6e1d6d41c29bd` (Apache-2.0; see `LICENSE`).

| File | SHA-256 |
| --- | --- |
| `eng.traineddata` | `7d4322bd2a7749724879683fc3912cb542f19906c83bcc1a52132556427170b2` |
| `chi_sim.traineddata` | `a5fcb6f0db1e1d6d8522f39db4e848f05984669172e584e8d76b6b3141e1f730` |
| `chi_sim_vert.traineddata` | `20590de84725bab69cde93bd6e8ed360a13cc5421a7e7364ddeb93e9af53d6da` |

Packaging places all three `.traineddata` files in `document-runtime/tessdata` beside `pab-document-worker`. Runtime OCR uses that directory explicitly and does not download data.
