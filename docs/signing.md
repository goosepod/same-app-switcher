# Windows release signing

Signing is prepared but disabled until a signing account is configured. Ordinary
CI builds and releases are unsigned while the repository variable `SIGN_RELEASES`
is unset. Do not set it to `true` until the steps below are complete.

The workflow uses [Azure Artifact Signing](https://learn.microsoft.com/en-us/azure/artifact-signing/)
(formerly Trusted Signing). Microsoft requires an account, identity validation,
and a certificate profile. Account creation, identity verification, billing, and
access grants must be completed by the account owner. No private key or Azure
client secret needs to be stored in GitHub.

## 1. Prepare Azure

1. Follow Microsoft's [Artifact Signing quickstart](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart).
   Create the signing account, complete identity validation, and create a
   **Public Trust** certificate profile for the publisher who will distribute the app.
2. Create an Entra application/service principal for this repository's signing job.
3. Grant it the **Artifact Signing Certificate Profile Signer** role, scoped to
   that certificate profile. It does not need subscription-wide Contributor access.
4. Configure an [OIDC federated identity credential](https://learn.microsoft.com/en-us/entra/workload-id/workload-identity-federation-create-trust)
   for the application with these exact values:

```json
{
  "name": "same-app-switcher-releases",
  "issuer": "https://token.actions.githubusercontent.com",
  "subject": "repo:goosepod/same-app-switcher:environment:release-signing",
  "audiences": ["api://AzureADTokenExchange"]
}
```

## 2. Configure GitHub

In repository **Settings → Environments**, create `release-signing`. Restrict its
deployment tags to `v*`, and add a required reviewer for signing releases. Protect
release tags and review workflow changes before allowing the job to run.

Add these environment variables under **release-signing → Environment variables**:

| Variable | Value |
|---|---|
| `AZURE_CLIENT_ID` | Application/client ID of the Entra application |
| `AZURE_TENANT_ID` | Entra directory/tenant ID |
| `AZURE_SUBSCRIPTION_ID` | Subscription containing the signing account |
| `SIGNING_ENDPOINT` | Exact regional endpoint shown for the signing account |
| `SIGNING_ACCOUNT_NAME` | Artifact Signing account name |
| `SIGNING_CERTIFICATE_PROFILE` | Public Trust certificate profile name |

Then add **repository** Actions variable `SIGN_RELEASES` with value `true`.
This particular variable must be repository-scoped: the workflow selects its
release job before environment variables become available.

The workflow grants OIDC token permission only to the signing job. Pull requests
and normal branch builds never run that job. When signing is enabled, a missing
configuration, invalid signature, or signing-service failure stops the release;
there is no fallback to publishing unsigned files.

## 3. Publish

Update the version in Cargo and the Windows resources, push the commit, and push
the matching version tag. For the current version:

```bash
git tag v0.3.0
git push origin v0.3.0
```

Approve the `release-signing` environment deployment when prompted. The job:

1. Builds and tests the native Windows app and installer action DLL.
2. Signs the EXE and DLL using SHA-256 with an RFC 3161 timestamp.
3. Embeds those signed payloads into the MSI, then signs the MSI.
4. Requires valid, timestamped Authenticode signatures for all three binaries.
5. Creates the ZIP from the signed EXE and computes checksums after signing.
6. Tests MSI installation, upgrade, startup cleanup, and uninstallation.
7. Uploads the signed artifact and publishes the GitHub Release.

The ordinary build artifact remains unsigned and is separate from the artifact
whose name ends in `-signed`. Download the release assets for signed distribution.

To inspect a downloaded file in PowerShell:

```powershell
Get-AuthenticodeSignature ./SameAppSwitcher-0.3.0-windows-x64.exe |
    Format-List Status, SignerCertificate, TimeStamperCertificate
```

Signing identifies the publisher and detects modified binaries. It does not
guarantee that SmartScreen will immediately trust a new publisher's downloads.
See the official [signing action](https://github.com/Azure/artifact-signing-action)
and [Microsoft signing integration documentation](https://learn.microsoft.com/en-us/azure/artifact-signing/how-to-signing-integrations)
for service configuration and troubleshooting.
