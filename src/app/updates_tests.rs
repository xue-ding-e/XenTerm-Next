use super::*;

const SHA: &str = "de031a32486d3bd9083c05a84de400cfdaa83360";
const RUNNING: &str = "xt-beta-f48f7e7-T1793094015";
const OFFERED: &str = "xt-beta-de031a3-T1794607373";
const SUFFIX: &str = "windows-x86_64.zip";

fn asset(name: String, contents: &[u8]) -> Asset {
    Asset {
        browser_download_url: format!("{DOWNLOAD_BASE}{name}"),
        name,
        state: "uploaded".into(),
        size: Some(contents.len() as u64),
        digest: Some(format!("sha256:{:x}", Sha256::digest(contents))),
    }
}

fn release() -> (Release, Vec<u8>, Vec<u8>) {
    let prefix = format!("{PREFIX}{SHA}-");
    let build = serde_json::to_vec(&serde_json::json!({
        "repository": REPOSITORY,
        "commit": SHA,
        "version_scheme": "xt-v1",
        "runtime_version": OFFERED,
        "version_timestamp": 1794607373u64,
        "platforms": ["windows-x86_64"],
    }))
    .unwrap();
    let build_asset = asset(format!("{prefix}BUILD.json"), &build);
    let package = asset(format!("{prefix}{SUFFIX}"), b"gpui");
    let sums = format!(
        "{}  {}\n{}  {}\n",
        build_asset.sha256().unwrap(),
        build_asset.name,
        package.sha256().unwrap(),
        package.name,
    )
    .into_bytes();
    let sums_asset = asset(format!("{prefix}SHA256SUMS"), &sums);
    let other = asset(format!("XenTerm-Next-Flutter-{SHA}-{SUFFIX}"), b"flutter");
    let stale = asset(
        format!("{PREFIX}{}-{SUFFIX}", "b".repeat(40)),
        b"stale gpui",
    );
    (
        Release {
            body: format!("源码 SHA: `{SHA}`"),
            name: "XenTerm Next GPUI beta (de031a32486d)".into(),
            tag_name: "beta".into(),
            html_url: RELEASE_PAGE.into(),
            prerelease: true,
            draft: false,
            assets: vec![other, stale, build_asset, package, sums_asset],
        },
        build,
        sums,
    )
}

fn check_with(current: &str, release: Release, build: Vec<u8>, sums: Vec<u8>) -> Result<Check> {
    check_release(current, release, Install::Archive, SUFFIX, |asset| {
        let data = if asset.name.ends_with("BUILD.json") {
            &build
        } else {
            &sums
        };
        let mut bytes = Vec::new();
        copy_verified(asset, data.as_slice(), &mut bytes, MAX_METADATA)?;
        Ok(bytes)
    })
}

#[test]
fn only_the_promoted_gpui_asset_can_be_offered() {
    let (release, build, sums) = release();
    let check = check_with(RUNNING, release, build, sums).unwrap();
    let Check::Available(update) = check else {
        panic!("GPUI update missing")
    };
    assert_eq!(update.version, OFFERED);
    assert_eq!(update.asset.name, format!("{PREFIX}{SHA}-{SUFFIX}"));
}

#[test]
fn flutter_release_is_rejected_before_reading_any_asset() {
    let (mut release, _, _) = release();
    release.tag_name = "flutter-beta-de031a32486d".into();
    let error = check_release(RUNNING, release, Install::Archive, SUFFIX, |_| {
        panic!("the Flutter release must never be read by GPUI");
    })
    .unwrap_err();
    assert!(error.to_string().contains("unexpected GPUI release"));
}

#[test]
fn flutter_asset_does_not_satisfy_a_missing_gpui_package() {
    let (mut release, build, sums) = release();
    release
        .assets
        .retain(|asset| asset.name != format!("{PREFIX}{SHA}-{SUFFIX}"));
    let error = check_with(RUNNING, release, build, sums).unwrap_err();
    assert!(error.to_string().contains("missing GPUI asset"));
}

#[test]
fn same_build_and_newer_local_version_are_both_current() {
    for running in [OFFERED, "xt-beta-aaaaaaa-T1794607444"] {
        let (release, build, sums) = release();
        assert!(matches!(
            check_with(running, release, build, sums).unwrap(),
            Check::Current
        ));
    }
}

#[test]
fn download_rejects_truncated_oversized_and_modified_payloads() {
    let asset = asset("package.zip".into(), b"abc");
    for invalid in [b"ab".as_slice(), b"abcd", b"abd"] {
        assert!(copy_verified(&asset, invalid, &mut Vec::new(), MAX_PACKAGE).is_err());
    }
    let mut output = Vec::new();
    copy_verified(&asset, b"abc".as_slice(), &mut output, MAX_PACKAGE).unwrap();
    assert_eq!(output, b"abc");
}

#[test]
#[ignore = "queries the public GitHub release and downloads the real platform archive"]
fn live_gpui_beta_download() {
    let Check::Available(update) = check_latest(RUNNING, Install::Archive).unwrap() else {
        panic!("no GPUI beta available for this test platform");
    };
    let directory = tempfile::tempdir().unwrap();
    let Download::Saved(path) = update.download(Some(directory.path())).unwrap() else {
        panic!("the portable package was not saved");
    };
    let length = path.metadata().unwrap().len();
    assert!(length > 0 && update.asset.size.is_none_or(|size| size == length));
    #[cfg(windows)]
    {
        let staged = tempfile::tempdir().unwrap();
        windows::prepare_portable(staged.path(), &path, &update.commit, &update.version).unwrap();
        assert!(staged.path().join("xenterm.exe").is_file());
    }
}
