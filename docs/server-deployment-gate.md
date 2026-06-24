# Finite Chat Server Deployment Gate

This is the server-side release gate for native app, TestFlight, and Friends
Alpha builds that use `https://chat.finite.computer`.

Finite Chat owns:

- the `finitechat-server` source and HTTP route contracts;
- the server build provenance exposed by `GET /health`;
- the compatibility decision for a finite-chat app/server pair;
- the release-blocking verification that production is running the expected
  server commit.

`../finitecomputer` owns the deployed host mechanics: host sync, backups,
Nix/k3s/Traefik, systemd service installation, `finited`, and production runtime
health. That split does not make server deployment optional for this repo. If an
app change depends on server behavior, stop and loop Paul into
`../finitecomputer` before distributing the app.

## Required Production Check

Before any phone or TestFlight build is handed to testers:

```sh
export FINITECHAT_RELEASE_COMMIT="$(git rev-parse --short=12 HEAD)"
cargo run -q -p finitechat-cli -- http --server https://chat.finite.computer health
```

The deployed health response must include:

```json
{
  "status": "ok",
  "server_version": "0.1.0",
  "source_commit": "<finite-chat commit>",
  "source_dirty": false
}
```

The release is blocked when any of these are true:

- `/health` omits `source_commit` or `server_version`;
- `source_commit` is not the finite-chat commit expected by the app build;
- `source_dirty` is `true`;
- a server-side route or DTO changed but production still reports an older
  compatible-looking build;
- the app requires a companion service change such as `push-drain`, blob
  storage policy, or Hermes bridge behavior that has not been deployed.

## Handoff To finitecomputer

When production needs a server update, loop Paul into `../finitecomputer` with:

- finite-chat branch and full commit SHA to deploy;
- whether the deployment needs only `finitechat-server` or also a companion
  worker such as `push-drain`;
- the finite-chat checks already run locally;
- any server data/backfill/rollback notes;
- the expected post-deploy `/health` payload.

The current finitecomputer deployment lane is documented in
`../finitecomputer/docs/finite-stack-deployment.md` and currently sketches:

```sh
just chat-server-deploy workspaces/ovh-fc-1 <finitechat-commit>
```

Treat the exact finitecomputer command as owned by that repo. The required
finite-chat acceptance criterion is that production `/health` reports the
expected finite-chat commit and the app-facing smoke tests pass against
`https://chat.finite.computer`.

## Post-Deploy Smoke

After Paul deploys the server, run:

```sh
cargo run -q -p finitechat-cli -- http --server https://chat.finite.computer health
cargo test -p finitechat-server --test http_routes
cargo test -p finitechat-server --test http_persistence
```

For Friends Alpha, continue with `docs/friends-alpha-integration-runbook.md`.
For TestFlight, continue with `docs/testflight-runbook.md`.
