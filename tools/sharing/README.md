# Capture sharing

Plan and implementation:

1. Validate in-memory and Parquet cropping against the same inclusive time window. Keep crossing scopes whole, sample rows inside the window, referenced frames (including module/address), and thread/process names. Normalize reversed bounds and never skip Parquet groups with unknown duration statistics.
2. Encode a complete `.orbit.zip` archive and a `.orbit.stream` for the static WASM viewer from one capture snapshot. Rebuild browser sampled stacks from the exact sample rows, including samples whose timeline spans aged out of the ring.
3. Add one-click **Share** / **Share slice**, service-side S3 uploads, encoded website links, download access to the complete archive, error reporting, and private-bucket deployment configuration.
4. Deploy the bucket and updated viewer, configure the service, then verify an actual upload and browser load from the website origin.

## Use

Stop recording. Select a time range by dragging on the timeline, then click
**Share slice** (or **Share** with no selection). The service uploads two files,
copies the website link, and exposes **Copy link** and **Open shared capture**.
The controls are also in **More** on narrow screens. Save → Selected slice
continues to download a local `.orbit.zip` without S3.

The website opens `viewer/index.html?capture=<encoded-stream-url>&download=<encoded-archive-url>`.
No Orbit service is needed by the recipient. The timeline and local sampling
reports work offline. **Download complete capture** preserves the Parquet
samples, callstack frame addresses/modules, event names, thread/process names,
target process and requested slice window. The browser stream is a presentation
of that archive; it does not expose service-only code/symbol lookup operations.

## Deploy

The AWS CLI must be installed on the machine running OrbitService. Credentials
stay on that machine: use its ordinary AWS profile, environment credentials or
instance/task role. The WASM viewer never receives write credentials.

### Install and authenticate the deployment CLI

Install [AWS CLI v2](https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html)
on the service machine and confirm `aws --version`. Use an administrator or
deployment identity for provisioning, separate from the restricted upload identity.
If your account uses IAM Identity Center:

```sh
aws configure sso --profile orbit-admin
aws sso login --profile orbit-admin
aws sts get-caller-identity --profile orbit-admin
```

Use the account's portal/start URL and SSO region in the interactive prompts.
If you use an existing IAM administrator access key instead, run
`aws configure --profile orbit-admin` and enter its key interactively.
The deploying identity needs CloudFormation, S3 bucket configuration and IAM
managed-policy creation permissions. The upload identity below does not.

### Create the bucket

Deploy the private bucket (replace the example origin and region).
For the main website use `WebsiteOrigin=https://orbitprofiler.dev`; if the
viewer is hosted on another origin, use that exact origin without a path:


```sh
aws cloudformation deploy --stack-name orbit-capture-sharing \
  --template-file tools/sharing/s3-stack.json --capabilities CAPABILITY_IAM \
  --parameter-overrides WebsiteOrigin=https://orbit.example RetentionDays=30 \
  --region us-west-2 --profile orbit-admin
aws cloudformation describe-stacks --stack-name orbit-capture-sharing \
  --query 'Stacks[0].Outputs' --region us-west-2 --profile orbit-admin
```

Attach the output `UploaderPolicyArn` to the service's IAM user or role.
It permits uploading, reading (for signed links), and cleaning up only under
`captures/`. The bucket blocks public access, uses encryption, permits browser
GET/HEAD from the configured website origin, and deletes shares after 30 days
(or the chosen RetentionDays). Account administrators perform IAM attachment.

### Configure upload credentials

On EC2 or ECS, attach `UploaderPolicyArn` to the instance/task role and let
AWS CLI use the role automatically; no access key or AWS_PROFILE is needed.
For a local workstation or another host, create a dedicated IAM upload user:

```sh
aws iam create-user --user-name orbit-sharing --profile orbit-admin
aws iam attach-user-policy --user-name orbit-sharing \
  --policy-arn YOUR_UPLOADER_POLICY_ARN --profile orbit-admin
```

In AWS Console → IAM → Users → orbit-sharing → Security credentials →
Create access key, select the CLI use case. Save the access key ID and secret
access key, then configure them on the machine running OrbitService:

```sh
aws configure --profile orbit-sharing
# Access key ID: the dedicated user's ID
# Secret access key: the dedicated user's secret
# Default region: us-west-2 (or the bucket's region)
# Default output: json
aws sts get-caller-identity --profile orbit-sharing
```

Enter keys at the prompts; keep them out of the repository and frontend.
AWS CLI stores this profile under the invoking user's `~/.aws/` directory.
Only attach the stack's restricted policy to this user; S3FullAccess and console
access are unnecessary. Rotate or delete the key from the IAM console when
appropriate. An SSO upload profile is also supported if its permission set
includes the upload policy, but shared URLs expire when that session's AWS
credentials expire.

### Configure and start OrbitService

Set these in the service's environment (including the region for the bucket):

```sh
export ORBIT_SHARE_BUCKET=the-stack-bucket-output
export ORBIT_SHARE_VIEWER_URL=https://orbit.example/viewer/index.html
export AWS_DEFAULT_REGION=us-west-2
export ORBIT_SHARE_EXPIRES_SECONDS=604800
# Optional if using a named local profile:
export AWS_PROFILE=orbit-sharing
```

The default signed links expire after seven days. Temporary AWS credentials
can make them expire sooner. S3 object retention is independent of link expiry.
For permanent links through an existing publicly readable CDN, set
`ORBIT_SHARE_PUBLIC_BASE_URL=https://captures.example` so `captures/<id>` is
readable there. This setting does not change bucket permissions or configure
a CDN. Do not set it for the private bucket without an authorized CDN origin.

Run `aws sts get-caller-identity` with the same identity and environment as the
service to verify it can find credentials. A successful identity check confirms
credentials are loaded; the first Share verifies the S3 permissions.

If OrbitService runs under sudo or systemd, its environment and home directory
can differ from your shell. Set `AWS_PROFILE=orbit-sharing`,
`AWS_CONFIG_FILE=/home/YOUR_USER/.aws/config`, and
`AWS_SHARED_CREDENTIALS_FILE=/home/YOUR_USER/.aws/credentials` in the service's
environment along with the ORBIT_SHARE variables. Ensure its OS user can read
those files and execute `aws` on PATH. Sudo must explicitly forward these
variables; exporting them in your shell alone is insufficient. Instance/task
roles avoid profile-file handling.

For a foreground build, after setting the environment above:

```sh
cargo build --release --manifest-path rust/crates/orbit-service/Cargo.toml
rust/crates/orbit-service/target/release/orbit-service --host 127.0.0.1 --serve 44766
```

Build the updated service and WASM viewer using the repository's build commands,
then rebuild and publish the main website with `tools/site/build_site.py`.
Use the exact public viewer URL in ORBIT_SHARE_VIEWER_URL. The website's existing
COOP/COEP headers can remain enabled: S3 GET responses must pass CORS for its
origin. Nothing new needs to run on the website server.

## Verify deployment

1. Record a capture with nested scopes and samples; stop, select a window
   crossing an outer scope, and click Share slice.
2. Open the copied link in another browser without a running service.
   Check timeline names, crossing scopes and Flat/Top-down/Bottom-up reports.
3. Download the archive; reopen it in Orbit and compare the same samples and
   referenced frames with the local Save → Selected slice archive.
4. Confirm the browser's capture GET succeeds with the website Origin and that
   an unsigned GET of the private S3 object returns 403. Check failed uploads
   report an error and allow retry.

`POST /api/capture/share?t0=<ns>&t1=<ns>` is also available to local automation;
both bounds are required together and reversed bounds are normalized. Requests
from a browser must originate from the service viewer. Errors return 400 for
invalid windows, 409 while recording, 503 for missing configuration, and 502
for upload failures. Partial publication is cleaned up on a best-effort basis;
S3 lifecycle handles objects left by interrupted processes.

AWS references: [S3 CORS](https://docs.aws.amazon.com/AmazonS3/latest/userguide/cors.html),
[AWS CLI uploads](https://docs.aws.amazon.com/cli/latest/reference/s3/cp.html),
[signed URL expiry](https://docs.aws.amazon.com/AmazonS3/latest/userguide/using-presigned-url.html).
