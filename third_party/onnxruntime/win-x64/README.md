# ONNX Runtime Windows x64 runtime

This directory contains the native runtime required by `ort` when the
`load-dynamic` feature is used.

Files:

- `onnxruntime.dll`
- `onnxruntime_providers_shared.dll`
- `hashes.json`

Source: ONNX Runtime 1.27.0 Windows x64 runtime, as used by the reference
Nuphus implementation. The runtime can be fetched with:

```powershell
pwsh ./scripts/fetch-runtime.ps1
```

The build script for `deskctl-cli` copies these files next to the built
`deskctl.exe` and into `target/<profile>/deps`.

Do not commit a different ONNX Runtime version without verifying compatibility
with `ort 2.0.0-rc.12` and updating `hashes.json`.

The ONNX Runtime license is distributed with the upstream package and must be
included in the final installer. See:

https://github.com/microsoft/onnxruntime/blob/main/LICENSE
