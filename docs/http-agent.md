# HTTP agent protocol (v1)

The same executable runs as an agent and as a coordinator. Build with `make`;
there are no additional runtime or Cargo dependencies. Start an agent on each
Linux machine with free V4L2 devices:

```sh
./validator-v4l2 serve --listen 0.0.0.0:8787
```

The default bind address is loopback. `--token TOKEN` or the
`VALIDATOR_HTTP_TOKEN` environment variable enables bearer authentication.
The coordinator accepts `--agent-token TOKEN` or the same environment variable.

Run from a workstation or either server; connect the SDI cable from the named
output on the transmitter to the named input on the receiver:

```sh
./validator-v4l2 list --agent-url http://sender:8787
./validator-v4l2 quick --tx-url http://sender:8787 --rx-url http://receiver:8787 \
  --pair /dev/video4=/dev/video1 --report quick.jsonl --html quick.html
./validator-v4l2 soak --tx-url http://sender:8787 --rx-url http://receiver:8787 \
  --pair /dev/video4=/dev/video1 --duration 86400 --seed 1 \
  --report soak.jsonl --html soak.html
```

For two boards on one machine, use the same agent URL on both sides. For a
single remote board, `soak --tx-url URL --device OUTPUT` checks output activity;
end-to-end validation requires a receiver. The coordinator gathers the mode
plan from the transmitter agent. Each case starts a transmitter task, then a
receiver task with matching settings. It collects both results and stops the
transmitter before moving on. Test traffic travels over SDI, not HTTP.

## Routes

All responses are JSON with `Content-Length`; each request uses a separate
HTTP connection. Optional authentication is `Authorization: Bearer TOKEN`.

| Method / route | Request | Result |
|---|---|---|
| GET `/v1/health` | none | API version, executable version, agent instance |
| GET `/v1/devices` | none | inventory JSONL in `stdout`, `stderr`, `exit_code` |
| POST `/v1/plan` | `args` array for `plan` | mode/feature plan in `stdout` |
| POST `/v1/jobs` | `command`, `args`, optional `run_id`, `lease_secs` | unique task `id` |
| GET `/v1/jobs` | none | retained task snapshots |
| GET `/v1/jobs/ID` | none | state, output, exit code, termination reason |
| POST `/v1/jobs/ID/heartbeat` | `lease_secs` | renewed task snapshot |
| POST `/v1/jobs/ID/stop` | none or `null` | stop requested; poll until complete |

Example task:

```json
{"command":"receive","args":["--device","/dev/video1","--mode","1080p25","--frames","250","--expect"],"run_id":"run-example","lease_secs":15}
```

A running task must receive heartbeats before its lease expires. The default
lease is 15 seconds; the permitted range is 1..120. The coordinator renews
leases once a second. When it disappears, the agent requests graceful stop,
then kills the child after two seconds if needed. Lease expiry is a failure
even if a gracefully stopped worker returns zero. Explicit transmitter stop
after reception is normal. Agent shutdown also stops active workers.

Tasks execute only validator `transmit`, `receive` or `software` commands,
without a shell. Hardware paths are restricted to `/dev/videoN`. Agent requests
cannot write arbitrary report/dump files or recursively start remote commands.
Each device is reserved while its task runs; a second task receives HTTP 409.
Limits: 16 active tasks, 32 retained snapshots, 64 KiB request bodies and
512 KiB per worker output stream. Output overflow stops the task.

JSONL and HTML reports are saved on the coordinator, so completed cases remain
available after agent snapshots are evicted. `run_id` joins agent jobs to the
coordinator's records. The original SSH flags remain supported for existing
scripts; HTTP agents are the primary distributed transport.
