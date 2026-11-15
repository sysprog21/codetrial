#!/usr/bin/env python3
# A model for scripts/commit-draft.py: it keeps every prompt it receives in
# DRAFT_LOG, one file per call, and answers each call with the next reply in
# DRAFT_REPLIES, separated by a line holding only "=====". The last reply
# repeats once the list runs out.
import os
from pathlib import Path
import sys

log = Path(os.environ["DRAFT_LOG"])
log.mkdir(exist_ok=True)
call = len(list(log.iterdir()))
(log / str(call)).write_text(sys.stdin.read())
replies = Path(os.environ["DRAFT_REPLIES"]).read_text().split("\n=====\n")
sys.stdout.write(replies[min(call, len(replies) - 1)])
