# Changes after the measured snapshot

The 50k and process-kill measurements use the source manifest and binary hashes in their reports. Later changes canonicalize UUID match-key strings, tighten the duplicate UPDATE predicates, expose per-sheet header metadata, retain ambiguous header rows for explicit confirmation, and continue bounded retention cleanup until the backlog drains.

Focused API tests verify UUID/duplicate behavior. Real browser tests verify header metadata. A real-parser API regression verifies header-looking rows remain visible, then explicit correction creates one product or exclusion removes a repeated header. A worker-dispatch regression verifies 1,200 expired raw rows drain through 500/500/200 continuations before the job finishes. These changes do not add product mutations to preparation.

The captured 50k workload uses explicit product UUIDs and staged native requests. No current-source binary identity or normal optimization-level-3 build claim is made.

## Live verification repairs

The live OCR smoke and follow-up review added strict source-currency validation, durable AI provenance, retained-remainder safeguards, semantic-question retention and PDF/headerless mapping recovery. Native apply helpers did not change. Final focused checks: 27 API cases, 27 bulk units and 418 Flow cases passed. See [live-repair.json](live-repair.json) for exact source hashes. No further 50k workload was run for this snapshot.
