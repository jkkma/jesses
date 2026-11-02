#!/usr/bin/env python3
"""Verify CPU-only compiled encoders without removing GPU processing support."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess

POLICY=json.loads(Path(__file__).with_name("cpu-encoding-policy.json").read_text(encoding="utf-8"))

def inspect_inventory(text):
    names=set(re.findall(r"^\s*[VAS][A-Z.]{5}\s+([a-zA-Z0-9_-]+)\b",text,re.MULTILINE))
    hardware=sorted(name for name in names if name in POLICY["excludedEncoders"] or any(name.endswith("_"+suffix) for suffix in POLICY["hardwareEncoderSuffixes"]))
    if hardware:raise ValueError(f"Hardware encoders are not permitted in Jesses: {', '.join(hardware)}")
    missing=sorted(set(POLICY["requiredSoftwareEncoders"])-names)
    if missing:raise ValueError(f"Required CPU encoders are absent: {', '.join(missing)}")
    return sorted(names)

def qualify(ffmpeg):
    result=subprocess.run([str(ffmpeg),"-hide_banner","-encoders"],capture_output=True,text=True,timeout=30)
    if result.returncode:raise ValueError(f"Compiled encoder inventory failed: {result.stderr[-2000:]}")
    names=inspect_inventory(result.stdout)
    with ffmpeg.open("rb") as binary:digest=hashlib.file_digest(binary,"sha256").hexdigest()
    return {"schemaVersion":1,"encodingPolicy":"cpu-only","ffmpegSha256":digest,"compiledEncoders":names,"hardwareEncoders":[],"gpuProcessingProhibited":False}

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("--ffmpeg",type=Path,required=True);parser.add_argument("--output",type=Path,required=True);args=parser.parse_args()
    result=qualify(args.ffmpeg.resolve(strict=True));args.output.write_text(json.dumps(result,indent=2)+"\n",encoding="utf-8");print(f"CPU encoder inventory verified: {args.output}; GPU processing remains independent.")

if __name__=="__main__":main()
