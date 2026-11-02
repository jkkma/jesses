#!/usr/bin/python3
"""Qualification-only real FFmpeg argv recorder and one-shot final mux fault.

Pass JESSES_TEST_REAL_FFMPEG (the qualified CPU provider) and a dedicated
JESSES_TEST_FILTER_INVOCATIONS JSONL file. Optional JESSES_TEST_FAIL_MUX names
an owned control file containing the exact native job ID to fail once.
"""
import json
import os
import pathlib
import sys

arguments = sys.argv[1:]
with open(os.environ['JESSES_TEST_FILTER_INVOCATIONS'], 'a') as log:
    log.write(json.dumps(arguments) + '\n')
control = os.environ.get('JESSES_TEST_FAIL_MUX')
if control:
    path = pathlib.Path(control)
    if path.is_file():
        job_id = path.read_text().strip()
        copying_video = any(arguments[index:index + 2] in (['-c:v', 'copy'], ['-c', 'copy']) for index in range(len(arguments)))
        if job_id and copying_video and any(job_id in value for value in arguments):
            path.unlink()
            print('Owned qualification fault: fail final mux after video checkpoint', file=sys.stderr)
            sys.exit(88)
real = os.environ['JESSES_TEST_REAL_FFMPEG']
os.execv(real, [real, *arguments])
