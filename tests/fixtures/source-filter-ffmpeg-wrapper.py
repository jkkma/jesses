#!/usr/bin/python3
"""Record FFmpeg arguments while forwarding to the qualified CPU provider."""
import json
import os
import sys

arguments = sys.argv[1:]
with open(os.environ['JESSES_TEST_FILTER_INVOCATIONS'], 'a') as log:
    log.write(json.dumps(arguments) + '\n')
real = os.environ['JESSES_TEST_REAL_FFMPEG']
os.execv(real, [real, *arguments])
