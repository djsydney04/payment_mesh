# Payment Mesh OAuth hosting

This Terraform root owns the `payment-mesh` Worker and its default
`payment-mesh.dylanmitic.workers.dev` hostname. It serves the Apple association
file and a safe fallback page at `/plaid/`. It does not host or proxy the Rust
payment API. There are no bank credentials in the Worker, and the fallback never
reflects, redirects, or forwards OAuth query parameters.

All deployment resources and Worker code are managed here with OpenTofu
**1.13.2** and Cloudflare provider **5.28.1**. Do not deploy this Worker with a
separate CLI. Shared Cloudflare resources remain in the central infrastructure
repository; this root uses the existing R2 bucket with its own state key:
`payment-mesh/cloudflare/terraform.tfstate`. Do not reuse the shared Cloudflare
or GitHub state keys. The dependency lockfile is committed; state and plans are
ignored.

## Required configuration

1. Obtain the actual Apple development team ID and confirm the app's bundle ID.
   From the repository root, generate the association and matching iOS settings:

   ```sh
   python3 scripts/configure-banking.py \
     --redirect-uri https://payment-mesh.dylanmitic.workers.dev/plaid/ \
     --team-id "$APPLE_TEAM_ID" \
     --bundle-id app.mesh.sandbox
   ```

2. Set `TF_VAR_apple_app_id` to the same `TEAMID.app.mesh.sandbox` identifier.
   The Terraform input has no invented default. Its precondition rejects AASA
   whose app identifier or allowed path differs from this configuration.
   `TF_VAR_aasa_file` may point at a different generated association file;
   relative paths are resolved from this directory.
   Keep the generated `configuration.json` beside that file. Terraform checks
   its exact redirect hostname/path and app identity so iOS entitlements cannot
   silently target a different host. Only AASA is published; this companion
   configuration stays local.
3. Supply `CLOUDFLARE_API_TOKEN` securely with account-scoped Workers Scripts
   write access (and any read permissions required by the provider).
   Supply `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` for Object Read & Write
   on the existing private `djsydney04-github-tfstate` R2 bucket. Do not put
   credentials in Terraform inputs, source, command arguments, or chat.
4. Confirm the Worker does not already have another deployment owner. If it
   already exists, import its script and subdomain into this root instead of
   creating a second owner. The account's `dylanmitic` subdomain is pre-existing
   and is intentionally not changed by this root.

## Validate, then review a deployment plan

Without credentials or backend access:

```sh
cd infra/cloudflare
tofu fmt -check
tofu init -backend=false -lockfile=readonly
tofu validate
node worker.test.mjs
```

The Worker tests use standard Request/Response APIs in Node 18+; they verify
routing, association content, HEAD, denied methods, and no OAuth parameter
reflection. They do not emulate Cloudflare deployment or Apple's association
cache. Configuration validation does not prove credentials, account ownership,
Apple team membership, or external bank acceptance.

Once the required identity and credentials are configured, initialize the real
backend and inspect the plan before applying:

```sh
tofu init -reconfigure -backend-config=state.s3.tfbackend -lockfile=readonly
tofu plan -lock-timeout=5m
tofu apply -lock-timeout=5m
```

Keep locking enabled; do not run concurrent applies. The reviewed plan should
only concern this Worker and its subdomain. No deployment has been performed by
adding these files.

After applying, verify a direct JSON response without redirects at
`https://payment-mesh.dylanmitic.workers.dev/.well-known/apple-app-site-association`.
Register the exact `https://payment-mesh.dylanmitic.workers.dev/plaid/` redirect
URI in Plaid's dashboard and use it as `PLAID_REDIRECT_URI` on the Rust server.
Build/sign the iOS app with the generated `.local/banking/ios.xcconfig` and test
Universal Links on a physical phone. The Rust API still needs its own reachable
HTTPS deployment before an internet-hosted pilot can work.

## References

- [Pinned Worker script schema](https://github.com/cloudflare/terraform-provider-cloudflare/blob/v5.28.1/docs/resources/workers_script.md)
- [Pinned Worker hostname schema](https://github.com/cloudflare/terraform-provider-cloudflare/blob/v5.28.1/docs/resources/workers_script_subdomain.md)
