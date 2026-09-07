@echo off
set PATH=F:\ainput\target\release;%PATH%
"F:\ainput\engine-gpu\x\sherpa-onnx-v1.12.39-cuda-12.x-cudnn-9.x-win-x64-cuda\bin\sherpa-onnx-offline.exe" --provider=cpu --num-threads=12 --qwen3-asr-conv-frontend=F:\ainput\models\qwen3-asr\conv_frontend.onnx --qwen3-asr-encoder=F:\ainput\models\qwen3-asr\encoder.int8.onnx --qwen3-asr-decoder=F:\ainput\models\qwen3-asr\decoder.int8.onnx --qwen3-asr-tokenizer=F:\ainput\models\qwen3-asr\tokenizer F:\ainput\models\qwen3-asr\test_wavs\codeswitch.wav > F:\ainput\engine-gpu\gpu-test.log 2>&1
echo DONE %ERRORLEVEL% >> F:\ainput\engine-gpu\gpu-test.log
