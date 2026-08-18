# MobileCLIP2-S0 vision encoder

`mobileclip2_s0_vision.onnx` is a converted image encoder from Apple's
MobileCLIP2-S0 model. It is used locally by SwiftScan and is never uploaded.

- Original project: https://github.com/apple/ml-mobileclip
- ONNX conversion: https://huggingface.co/plhery/mobileclip2-onnx
- Model license: Apple ML Research Model Terms of Use (`apple-amlr`)
- SHA-256: `13D20EBFA8A8F63890EB2727FE4DC63009FF970F43E0F7D9D2ED999659F70C8A`

The model artifact remains subject to the original model terms. The surrounding
SwiftScan source code is not relicensed by this notice.

`runtime/onnxruntime.dll` and `runtime/onnxruntime_providers_shared.dll` are
distributed by Microsoft in `Microsoft.ML.OnnxRuntime.DirectML` under the MIT
license: https://github.com/microsoft/onnxruntime/blob/main/LICENSE

Runtime SHA-256 checksums:

- `onnxruntime.dll`: `A2323BC49544645B911743052F1EDCE594E17DF1E3423B71468C7386BC902F80`
- `onnxruntime_providers_shared.dll`: `8B33B30AC866C938AA3D946D4F92FC2BA70FFF06EF45D5CE22E483F19BA2C896`
