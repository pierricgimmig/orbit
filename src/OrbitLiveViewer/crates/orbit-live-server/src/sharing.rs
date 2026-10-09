//! S3 uploads happen on the service machine, using its AWS credential chain.
//! No credentials or write URLs are exposed to the browser.
use rand::RngCore;
use serde::Serialize;
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Clone, Debug)]
pub struct ShareConfig {
    bucket: String,
    viewer_url: String,
    public_base: Option<String>,
    expires: u32,
}

#[derive(Debug, Serialize)]
pub struct ShareResult {
    pub viewer_url: String,
    pub download_url: String,
    pub expires_in: Option<u32>,
}

impl ShareConfig {
    pub fn from_env() -> Result<Self, String> {
        let bucket = std::env::var("ORBIT_SHARE_BUCKET").map_err(|_| {
            "Set ORBIT_SHARE_BUCKET and ORBIT_SHARE_VIEWER_URL on the service to enable sharing"
        })?;
        if bucket.is_empty()
            || !bucket
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'.')
        {
            return Err("ORBIT_SHARE_BUCKET must be an S3 bucket name".into());
        }
        let viewer_url = std::env::var("ORBIT_SHARE_VIEWER_URL")
            .map_err(|_| "Set ORBIT_SHARE_VIEWER_URL to the website's viewer/index.html URL")?;
        validate_base(&viewer_url)?;
        let public_base = std::env::var("ORBIT_SHARE_PUBLIC_BASE_URL").ok();
        if let Some(base) = &public_base {
            validate_base(base)?;
        }
        let expires = std::env::var("ORBIT_SHARE_EXPIRES_SECONDS")
            .unwrap_or_else(|_| "604800".into())
            .parse::<u32>()
            .map_err(|_| "Invalid ORBIT_SHARE_EXPIRES_SECONDS")?;
        if !(60..=604800).contains(&expires) {
            return Err("Share expiry must be between 60 and 604800 seconds".into());
        }
        Ok(Self {
            bucket,
            viewer_url,
            public_base,
            expires,
        })
    }

    pub fn upload(&self, bundle: Vec<u8>, stream: Vec<u8>) -> Result<ShareResult, String> {
        let mut random = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut random);
        let id: String = random.iter().map(|b| format!("{b:02x}")).collect();
        self.publish(&id, bundle, stream, &Aws)
    }

    fn publish(
        &self,
        id: &str,
        bundle: Vec<u8>,
        stream: Vec<u8>,
        store: &impl ObjectStore,
    ) -> Result<ShareResult, String> {
        let archive = format!("captures/{id}.orbit.zip");
        let clip = format!("captures/{id}.orbit.stream");
        store.put(&self.bucket, &archive, &bundle, "application/zip")?;
        if let Err(e) = store.put(&self.bucket, &clip, &stream, "application/octet-stream") {
            store.delete(&self.bucket, &archive);
            store.delete(&self.bucket, &clip);
            return Err(e);
        }
        let result = (|| {
            let url = |key: &str| match &self.public_base {
                Some(base) => Ok(format!("{}/{key}", base.trim_end_matches('/'))),
                None => store.presign(&self.bucket, key, self.expires),
            };
            let capture_url = url(&clip)?;
            let download_url = url(&archive)?;
            Ok(ShareResult {
                viewer_url: format!(
                    "{}?capture={}&download={}",
                    self.viewer_url,
                    percent_encode(&capture_url),
                    percent_encode(&download_url)
                ),
                download_url,
                expires_in: self.public_base.is_none().then_some(self.expires),
            })
        })();
        if result.is_err() {
            store.delete(&self.bucket, &archive);
            store.delete(&self.bucket, &clip);
        }
        result
    }
}

fn validate_base(url: &str) -> Result<(), String> {
    if !url.starts_with("https://")
        || url.len() <= 8
        || url.contains(['?', '#'])
        || url.chars().any(char::is_whitespace)
    {
        return Err("Sharing URLs must be HTTPS URLs without a query or fragment".into());
    }
    Ok(())
}

pub fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    encoded
}

trait ObjectStore {
    fn put(&self, bucket: &str, key: &str, bytes: &[u8], content_type: &str) -> Result<(), String>;
    fn presign(&self, bucket: &str, key: &str, expires: u32) -> Result<String, String>;
    fn delete(&self, bucket: &str, key: &str);
}
struct Aws;
fn aws() -> Command {
    let mut cmd = Command::new("aws");
    cmd.args(["--cli-connect-timeout", "10", "--cli-read-timeout", "60"])
        .env("AWS_PAGER", "")
        .env("AWS_MAX_ATTEMPTS", "2");
    cmd
}
fn aws_result(output: std::process::Output) -> Result<String, String> {
    if !output.status.success() {
        // Never echo subprocess output: a presign error can include credential details.
        return Err(format!("S3 operation failed ({}); check the service's AWS credentials, bucket, region and permissions", output.status));
    }
    String::from_utf8(output.stdout)
        .map(|s| s.trim().to_string())
        .map_err(|_| "Invalid AWS CLI response".into())
}
impl ObjectStore for Aws {
    fn put(&self, bucket: &str, key: &str, bytes: &[u8], content_type: &str) -> Result<(), String> {
        let mut child = aws()
            .args([
                "s3",
                "cp",
                "-",
                &format!("s3://{bucket}/{key}"),
                "--only-show-errors",
                "--content-type",
                content_type,
                "--expected-size",
                &bytes.len().to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Cannot run AWS CLI; install aws on the service machine".to_string())?;
        let written = child
            .stdin
            .take()
            .ok_or("AWS CLI stdin unavailable")?
            .write_all(bytes);
        let output = child
            .wait_with_output()
            .map_err(|_| "Cannot wait for AWS CLI")?;
        aws_result(output)?;
        written.map_err(|_| "S3 upload interrupted".to_string())
    }
    fn presign(&self, bucket: &str, key: &str, expires: u32) -> Result<String, String> {
        let output = aws()
            .args([
                "s3",
                "presign",
                &format!("s3://{bucket}/{key}"),
                "--expires-in",
                &expires.to_string(),
            ])
            .stderr(Stdio::null())
            .output()
            .map_err(|_| "Cannot run AWS CLI")?;
        let url = aws_result(output)?;
        if !url.starts_with("https://") {
            return Err("AWS CLI returned a non-HTTPS download URL".into());
        }
        Ok(url)
    }
    fn delete(&self, bucket: &str, key: &str) {
        let _ = aws()
            .args([
                "s3",
                "rm",
                &format!("s3://{bucket}/{key}"),
                "--only-show-errors",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    #[derive(Default)]
    struct Fake {
        objects: RefCell<Vec<(String, Vec<u8>)>>,
        fail_stream: bool,
    }
    impl ObjectStore for Fake {
        fn put(&self, _: &str, key: &str, bytes: &[u8], _: &str) -> Result<(), String> {
            if self.fail_stream && key.ends_with("stream") {
                return Err("upload failed".into());
            }
            self.objects.borrow_mut().push((key.into(), bytes.into()));
            Ok(())
        }
        fn presign(&self, _: &str, key: &str, _: u32) -> Result<String, String> {
            Ok(format!(
                "https://bucket.s3.amazonaws.com/{key}?X-Amz-Signature=a&b=c"
            ))
        }
        fn delete(&self, _: &str, key: &str) {
            self.objects.borrow_mut().retain(|(k, _)| k != key);
        }
    }
    fn config() -> ShareConfig {
        ShareConfig {
            bucket: "bucket".into(),
            viewer_url: "https://orbit.example/viewer/index.html".into(),
            public_base: None,
            expires: 3600,
        }
    }
    #[test]
    fn uploads_archive_and_clip_and_encodes_signed_urls() {
        let store = Fake::default();
        let result = config()
            .publish("id", vec![1, 2], vec![3, 4], &store)
            .unwrap();
        assert_eq!(store.objects.borrow()[0].1, vec![1, 2]);
        assert_eq!(store.objects.borrow()[1].1, vec![3, 4]);
        assert!(result.viewer_url.contains("%3FX-Amz-Signature%3Da%26b%3Dc"));
        assert!(result.viewer_url.contains("&download="));
        assert_eq!(result.expires_in, Some(3600));
    }
    #[test]
    fn cleans_up_archive_if_second_upload_fails() {
        let store = Fake {
            fail_stream: true,
            ..Fake::default()
        };
        assert!(config().publish("id", vec![1], vec![2], &store).is_err());
        assert!(store.objects.borrow().is_empty());
    }
    #[test]
    fn durable_public_links_have_no_expiry() {
        let mut config = config();
        config.public_base = Some("https://cdn.example/".into());
        let result = config
            .publish("id", vec![], vec![], &Fake::default())
            .unwrap();
        assert_eq!(
            result.download_url,
            "https://cdn.example/captures/id.orbit.zip"
        );
        assert_eq!(result.expires_in, None);
    }
}
