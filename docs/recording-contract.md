# Recording contract

## Provisioning status

Not provisioned. Before `CODETRIAL_RECORDING_ENABLED` can be introduced, an
operator records the following values outside the repository and runs
`./scripts/recording-provision-check.sh` with them in its environment:

| Environment variable | Required value |
|---|---|
| `CODETRIAL_RECORDING_GCS_BUCKET` | Private staging bucket name |
| `CODETRIAL_RECORDING_DRIVE_ID` | Platform-owned Shared Drive ID |
| `CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON` | Service-account key JSON, supplied from a secret store |
| `CODETRIAL_RECORDING_TEMPLATE_BASE_URL` | Public HTTPS origin that will serve `/recording/index.html` |

Record the LiveKit Cloud project ID, service-account principal, its exact roles,
the bucket lifecycle-policy date, Shared Drive creation date, and all role-grant
dates in the private operations record. Do not put IDs or key material in this
repository.

Grant the workload service account exactly these data-plane roles, scoped only
to the named resources: `roles/storage.objectAdmin` on the staging bucket and
the `organizer` member role on the dedicated Shared Drive. The former permits
object list/read/write/delete but not bucket administration; the latter is
needed to create reader permissions and delete the delivered artifact. Do not
grant it a project-wide Storage role, Workspace administrator privilege, or any
other Shared Drive membership.

The checker performs only a basic HTTPS-origin check (including no loopback or
bracketed IPv6 host) and does not establish public reachability or fetch the
path before task 8a creates it. The operator records that evidence separately;
task 7a verifies the template URL in a real Egress run. The checker writes the
supplied key and short-lived access-token header only to a private temporary
directory, removes the key from its child-process environment, and does not
replace a developer's active `gcloud` account.

## Fixed paths

The later provider-contract task owns these fixed values:

- Webhook route: `POST /api/recording/webhook`
- Template path: `/recording/index.html`
- GCS object path: `{prefix}/{recording_id}.mp4`
