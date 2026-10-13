use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const PORTAL_UPLOAD_URL: &str = "https://central.sonatype.com/api/v1/publisher/upload";
const PORTAL_STATUS_URL: &str = "https://central.sonatype.com/api/v1/publisher/status";
const FIXED_CATALOG_ARTIFACT: &str = "share-catalog";

#[derive(Debug, Serialize, Deserialize, Clone)]
struct PublishRequest {
    #[serde(alias = "filePath")]
    file_path: String,
    #[serde(alias = "groupId")]
    group_id: String,
    #[serde(alias = "artifactId")]
    artifact_id: String,
    version: String,
    #[serde(default = "default_packaging")]
    packaging: String,
    #[serde(default)]
    classifier: String,
    #[serde(default, alias = "portalUsername")]
    portal_username: String,
    #[serde(default, alias = "portalToken")]
    portal_token: String,
    #[serde(default = "default_true", alias = "publishCatalog")]
    publish_catalog: bool,
    #[serde(default, alias = "licenseName")]
    license_name: String,
    #[serde(default, alias = "licenseUrl")]
    license_url: String,
    #[serde(default, alias = "scmUrl")]
    scm_url: String,
    #[serde(default, alias = "devId")]
    dev_id: String,
    #[serde(default, alias = "devName")]
    dev_name: String,
    #[serde(default, alias = "devEmail")]
    dev_email: String,
}

fn default_packaging() -> String {
    "jar".to_string()
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct CatalogEntry {
    group_id: String,
    artifact_id: String,
    version: String,
    file_name: String,
    packaging: String,
    classifier: String,
    size: u64,
    sha256: String,
    published_at: String,
    deployment_id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
struct CatalogFile {
    group_id: String,
    artifact_id: String,
    updated_at: String,
    latest_version: Option<String>,
    entries: Vec<CatalogEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct HistoryEntry {
    kind: String,
    group_id: String,
    artifact_id: String,
    version: String,
    file_name: String,
    size: u64,
    sha256: String,
    published_at: String,
    deployment_id: String,
    status: String,
}

#[derive(Debug, Serialize)]
struct EnvInfo {
    gpg_available: bool,
    gpg_version: String,
    app_data_dir: String,
}

#[derive(Debug, Serialize)]
struct DryRunResult {
    bundle_path: String,
    files: Vec<String>,
    sha256: String,
    size: u64,
    pom_preview: String,
}

#[derive(Debug, Serialize)]
struct PublishResult {
    deployment_id: String,
    status: String,
    catalog_version: Option<String>,
    catalog_status: Option<String>,
}

// ---------- Gav 校验 ----------
fn is_gav_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' 
}

pub fn validate_gav(group_id: &str, artifact_id: &str, version: &str) -> Result<(), String> {
    let g = group_id.trim();
    let a = artifact_id.trim();
    let v = version.trim();
    if g.is_empty() {
        return Err("groupId 不能为空（例：com.example）".to_string());
    }
    if a.is_empty() {
        return Err("artifactId 不能为空（例：my-lib）".to_string());
    }
    if v.is_empty() {
        return Err("version 不能为空（例：1.0.0）".to_string());
    }
    for (name, s) in [("groupId", g), ("artifactId", a), ("version", v)] {
        if s.contains(' ') || s.contains('\u{3000}') {
            return Err(format!("{name} 不能包含空格"));
        }
        if !s.chars().all(is_gav_char) {
            return Err(format!("{name} 只能包含英文字母、数字、点(.)、横线(-)、下划线(_)"));
        }
    }
    if g.starts_with('.') || g.starts_with('-') || g.ends_with('.') || g.ends_with('-') {
        return Err("groupId 不能以点号或横线开头/结尾".to_string());
    }
    if a.starts_with('.') || a.starts_with('-') || a.ends_with('.') || a.ends_with('-') {
        return Err("artifactId 不能以点号或横线开头/结尾".to_string());
    }
    if v.eq_ignore_ascii_case("latest") {
        return Err("version 不能是 latest，请使用具体版本号".to_string());
    }
    Ok(())
}

pub fn gav_warnings(group_id: &str, version: &str) -> Vec<String> {
    let mut w = Vec::new();
    if !group_id.contains('.') {
        w.push("groupId 没有点号：正式版要求是你在 Sonatype 验证过的域名（如 com.example），快照测试除外。".to_string());
    }
    if version.ends_with("-SNAPSHOT") {
        w.push("检测到 -SNAPSHOT 快照版：适合联调，正式发布请使用 release 版本号。".to_string());
    } else {
        w.push("正式版将强制校验 GPG 签名与 sources/javadoc，本工具会自动生成，请确保已安装 Gpg4win 并建好密钥。".to_string());
    }
    w
}

pub fn maven_dir(group_id: &str) -> String {
    group_id.replace('.', "/")
}

pub fn extension_for(packaging: &str, input: &Path) -> String {
    match packaging {
        "tar.gz" => "tar.gz".to_string(),
        "jar" | "war" | "pom" | "zip" | "json" | "xml" => packaging.to_string(),
        other if !other.is_empty() => other.to_string(),
        _ => input
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("jar")
            .to_string(),
    }
}

pub fn artifact_file_name(artifact: &str, version: &str, classifier: &str, ext: &str) -> String {
    if classifier.trim().is_empty() {
        format!("{artifact}-{version}.{ext}")
    } else {
        format!("{artifact}-{version}-{}.{ext}", classifier.trim())
    }
}

pub fn maven_rel_path(group_id: &str, artifact: &str, version: &str, file_name: &str) -> String {
    format!("{}/{}/{}/{}", maven_dir(group_id), artifact, version, file_name)
}

pub fn license_url_for(name: &str) -> &'static str {
    match name {
        "Apache-2.0" => "https://www.apache.org/licenses/LICENSE-2.0",
        "BSD-3-Clause" => "https://opensource.org/licenses/BSD-3-Clause",
        "GPL-3.0-only" => "https://www.gnu.org/licenses/gpl-3.0.html",
        "MPL-2.0" => "https://opensource.org/licenses/MPL-2.0",
        _ => "https://opensource.org/licenses/MIT",
    }
}

pub fn license_display(name: &str) -> &'static str {
    match name {
        "Apache-2.0" => "Apache License 2.0",
        "BSD-3-Clause" => "BSD 3-Clause License",
        "GPL-3.0-only" => "GNU General Public License v3.0 only",
        "MPL-2.0" => "Mozilla Public License 2.0",
        _ => "MIT License",
    }
}

fn xml_esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            _ => o.push(c),
        }
    }
    o
}

#[derive(Debug, Clone, Default)]
pub struct PomMeta {
    pub license_name: String,
    pub license_url: String,
    pub scm_url: String,
    pub dev_id: String,
    pub dev_name: String,
    pub dev_email: String,
}

impl PomMeta {
    fn from_request(r: &PublishRequest) -> Self {
        let raw_name = r.license_name.trim();
        let license_name = if raw_name.is_empty() { "MIT".to_string() } else { raw_name.to_string() };
        let license_url = if r.license_url.trim().is_empty() {
            license_url_for(&license_name).to_string()
        } else {
            r.license_url.trim().to_string()
        };
        let dev_name = r.dev_name.trim().to_string();
        let dev_id = if r.dev_id.trim().is_empty() { dev_name.clone() } else { r.dev_id.trim().to_string() };
        Self {
            license_name,
            license_url,
            scm_url: r.scm_url.trim().to_string(),
            dev_id,
            dev_name,
            dev_email: r.dev_email.trim().to_string(),
        }
    }
}

fn validate_pom_meta(r: &PublishRequest) -> Result<PomMeta, String> {
    let m = PomMeta::from_request(r);
    if m.scm_url.is_empty() {
        return Err("SCM 地址未填：中央仓要求 pom 里有 <scm><url>（本次报 SCM URL is not defined）。请在表单“SCM 地址”里填你的仓库地址（如 https://github.com/xxx/yyy）后再试。".to_string());
    }
    if !(m.scm_url.starts_with("http://") || m.scm_url.starts_with("https://")) {
        return Err("SCM 地址格式不对：请填 http(s):// 开头的仓库地址。".to_string());
    }
    if m.dev_name.is_empty() {
        return Err("开发者姓名未填：中央仓要求 pom 里有 developers 信息（本次报 Developers information is missing）。请在表单中填写后再试。".to_string());
    }
    if !m.dev_email.contains('@') {
        return Err("开发者邮箱未填或格式不对：需要含 @ 的有效邮箱。".to_string());
    }
    Ok(m)
}

pub fn build_minimal_pom(group_id: &str, artifact_id: &str, version: &str, packaging: &str, meta: &PomMeta) -> String {
    let packaging = if packaging == "tar.gz" { "tar.gz" } else { packaging };
    let (g, a, v, pk) = (xml_esc(group_id), xml_esc(artifact_id), xml_esc(version), xml_esc(packaging));
    let (ln, lu, scm, di, dn, de) = (
        xml_esc(license_display(&meta.license_name)),
        xml_esc(&meta.license_url),
        xml_esc(&meta.scm_url),
        xml_esc(&meta.dev_id),
        xml_esc(&meta.dev_name),
        xml_esc(&meta.dev_email),
    );
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:schemaLocation="http://maven.apache.org/POM/4.0.0 http://maven.apache.org/xsd/maven-4.0.0.xsd">
  <modelVersion>4.0.0</modelVersion>
  <groupId>{g}</groupId>
  <artifactId>{a}</artifactId>
  <version>{v}</version>
  <packaging>{pk}</packaging>
  <name>{a}</name>
  <description>{a} published by MavenShare</description>
  <url>https://central.sonatype.com/</url>
  <licenses>
    <license>
      <name>{ln}</name>
      <url>{lu}</url>
      <distribution>repo</distribution>
    </license>
  </licenses>
  <scm>
    <connection>scm:git:{scm}</connection>
    <developerConnection>scm:git:{scm}</developerConnection>
    <url>{scm}</url>
  </scm>
  <developers>
    <developer>
      <id>{di}</id>
      <name>{dn}</name>
      <email>{de}</email>
    </developer>
  </developers>
</project>
"#
    )
}

pub fn next_catalog_version(latest: Option<&str>) -> String {
    match latest {
        None => "1.0.0".to_string(),
        Some(v) => {
            let parts: Vec<&str> = v.split('.').collect();
            if let Some(last) = parts.last() {
                if let Ok(n) = last.parse::<u64>() {
                    let mut p = parts.clone();
                    let len = p.len();
                    p[len - 1] = "";
                    let mut s = p[..len - 1].join(".");
                    if !s.is_empty() {
                        s.push('.');
                    }
                    return format!("{s}{}", n + 1);
                }
            }
            format!("{v}.1")
        }
    }
}

// ---------- 文件 / 签名 ----------
pub fn sha256_file(path: &Path) -> Result<String, String> {
    let data = fs::read(path).map_err(|e| format!("读取文件失败：{e}"))?;
    let mut h = Sha256::new();
    h.update(&data);
    Ok(hex_of(h.finalize()))
}

fn hex_of(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

fn gpg_candidates() -> Vec<&'static str> {
    let mut v = vec!["gpg", "gpg.exe"];
    for dir in ["C:\\Program Files (x86)\\GnuPG\\bin\\gpg.exe", "C:\\Program Files\\GnuPG\\bin\\gpg.exe"] {
        if Path::new(dir).exists() {
            v.push(dir);
        }
    }
    v
}

pub fn gpg_version() -> Option<String> {
    for bin in gpg_candidates() {
        if let Ok(out) = Command::new(bin).arg("--version").output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).to_string();
                let first = s.lines().next().unwrap_or("gpg").to_string();
                return Some(first);
            }
        }
    }
    None
}

pub fn sign_file(path: &Path) -> Result<PathBuf, String> {
    let asc = path.with_extension(format!(
        "{}.asc",
        path.extension().and_then(|s| s.to_str()).unwrap_or("bin")
    ));
    let asc_str = asc.to_string_lossy().to_string();
    let target = path.to_string_lossy().to_string();
    let mut last_err = String::new();
    for bin in gpg_candidates() {
        let out = Command::new(bin)
            .args(["--yes", "--armor", "--detach-sign", "--output", &asc_str, &target])
            .output();
        match out {
            Ok(o) if o.status.success() && asc.exists() => return Ok(asc),
            Ok(o) => {
                last_err = String::from_utf8_lossy(&o.stderr).to_string();
            }
            Err(e) => {
                last_err = e.to_string();
            }
        }
    }
    Err(format!(
        "GPG 签名失败：{last_err}。请先在“0 · GPG 安装与密钥”一键安装并生成密钥，再重试。"
    ))
}

fn build_empty_jar(path: &Path, artifact: &str, version: &str, kind: &str) -> Result<(), String> {
    let f = fs::File::create(path).map_err(|e| format!("创建 {kind} 失败：{e}"))?;
    let mut zip = ZipWriter::new(f);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("META-INF/MANIFEST.MF", opts)
        .map_err(|e| format!("写入 manifest 失败：{e}"))?;
    zip.write_all(format!("Manifest-Version: 1.0\nCreated-By: MavenShare\nBundle-Kind: {kind}\n").as_bytes())
        .map_err(|e| format!("写入 manifest 失败：{e}"))?;
    if kind == "sources" {
        zip.start_file("README.txt", opts)
            .map_err(|e| format!("写入 sources 占位失败：{e}"))?;
        zip.write_all(format!("{artifact}-{version} auto generated sources placeholder by MavenShare.\n").as_bytes())
            .map_err(|e| format!("写入 sources 占位失败：{e}"))?;
    } else {
        zip.start_file("index.html", opts)
            .map_err(|e| format!("写入 javadoc 占位失败：{e}"))?;
        zip.write_all(format!("<html><body><h1>{artifact} {version}</h1><p>Auto generated javadoc placeholder by MavenShare.</p></body></html>").as_bytes())
            .map_err(|e| format!("写入 javadoc 占位失败：{e}"))?;
    }
    zip.finish().map_err(|e| format!("打包 {kind} 失败：{e}"))?;
    Ok(())
}

// ---------- Bundle 打包 ----------

// ---------- 产物内容校验：归档类 packaging 必须是标准归档 ----------
// 中央仓在 building manifests 阶段会逐个解析 bundle 内文件；
// 若主产物（如 .jar）实际不是 ZIP（例如 .exe 或带启动脚本的可执行 jar），
// Portal 即报 No Archiver found for the stream signature 并判 FAILED。
fn sniff_kind(data: &[u8]) -> &'static str {
    if data.is_empty() {
        return "empty";
    }
    if data.starts_with(b"PK\x03\x04") || data.starts_with(b"PK\x05\x06") || data.starts_with(b"PK\x07\x08") {
        return "zip";
    }
    if data.starts_with(b"\x1f\x8b") {
        return "gzip";
    }
    if data.starts_with(b"MZ") {
        return "exe";
    }
    if data.starts_with(b"#!") {
        return "script";
    }
    "unknown"
}

fn kind_desc_of(kind: &str) -> &'static str {
    match kind {
        "zip" => "标准 ZIP 归档（jar/war/zip）",
        "gzip" => "GZip 压缩包（tar.gz）",
        "exe" => "Windows 可执行文件 .exe（不是 jar）",
        "script" => "带启动脚本的可执行文件（中央仓不接受）",
        "empty" => "空文件",
        _ => "未知格式",
    }
}

fn sniff_file_head(path: &Path) -> &'static str {
    use std::io::Read;
    let mut head = [0u8; 8];
    match fs::File::open(path).and_then(|mut f| f.read(&mut head)) {
        Ok(n) => sniff_kind(&head[..n]),
        Err(_) => "unknown",
    }
}

fn validate_artifact_bytes(resolved_ext: &str, data: &[u8], source: &Path) -> Result<(), String> {
    let name = source.file_name().and_then(|s| s.to_str()).unwrap_or("待发布文件").to_string();
    if data.is_empty() {
        return Err(format!("待发布文件“{name}”是空文件（0 字节），无法发布。"));
    }
    let ext_l = resolved_ext.to_ascii_lowercase();
    let zip_like = matches!(ext_l.as_str(), "jar" | "war" | "zip" | "aar" | "ear" | "rar");
    if zip_like {
        if !data.starts_with(b"PK") {
            let kind = sniff_kind(data);
            let desc = kind_desc_of(kind);
            return Err(format!("待发布文件“{name}”不是有效的 {resolved_ext}：文件头识别为“{desc}”，缺少 ZIP 魔数 PK。Maven 中央仓会在 building manifests 阶段报 No Archiver found for the stream signature 并判 FAILED。常见原因：把 .exe 程序或带启动脚本的可执行 jar 当成了普通 jar。请换成标准 ZIP 格式的 {resolved_ext} 文件，或把 packaging 改成与文件真实格式一致的类型后再试打包。"));
        }
        if zip::ZipArchive::new(std::io::Cursor::new(data)).is_err() {
            return Err(format!("待发布文件“{name}”虽以 PK 开头，但 ZIP 中央目录解析失败（文件可能损坏或不完整），中央仓同样无法处理。请用完好的 {resolved_ext} 文件重试。"));
        }
        return Ok(());
    }
    if ext_l == "tar.gz" || ext_l == "tgz" {
        if !data.starts_with(b"\x1f\x8b") {
            let kind = sniff_kind(data);
            let desc = kind_desc_of(kind);
            return Err(format!("待发布文件“{name}”不是有效的 {resolved_ext}：缺少 GZip 魔数（文件头识别为“{desc}”）。请检查文件后再试打包。"));
        }
        return Ok(());
    }
    Ok(())
}

#[cfg(test)]
mod content_tests {
    use super::*;
    #[test]
    fn exe_rejected_as_jar() {
        let data = b"MZ\x90\x00test-binary";
        let err = validate_artifact_bytes("jar", data, Path::new("app.exe")).unwrap_err();
        assert!(err.contains("No Archiver"), "unexpected msg: {err}");
    }
    #[test]
    fn launch_script_jar_rejected() {
        let data = b"#!/bin/bash\necho hi";
        assert!(validate_artifact_bytes("jar", data, Path::new("app.jar")).is_err());
    }
    #[test]
    fn empty_rejected() {
        assert!(validate_artifact_bytes("jar", b"", Path::new("a.jar")).is_err());
    }
    #[test]
    fn minimal_zip_accepted() {
        let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        w.start_file("a.txt", SimpleFileOptions::default()).unwrap();
        w.write_all(b"hi").unwrap();
        let data = w.finish().unwrap().into_inner();
        assert!(validate_artifact_bytes("jar", &data, Path::new("a.jar")).is_ok());
    }
    #[test]
    fn gzip_magic_ok_and_non_gzip_rejected() {
        assert!(validate_artifact_bytes("tar.gz", &[0x1f, 0x8b, 0x08, 0x00], Path::new("a.tar.gz")).is_ok());
        assert!(validate_artifact_bytes("tar.gz", b"PK\x03\x04xx", Path::new("a.tar.gz")).is_err());
    }
    #[test]
    fn sniff_table() {
        assert_eq!(sniff_kind(b"PK\x03\x04x"), "zip");
        assert_eq!(sniff_kind(b"\x1f\x8bx"), "gzip");
        assert_eq!(sniff_kind(b"MZx"), "exe");
        assert_eq!(sniff_kind(b"#!x"), "script");
        assert_eq!(sniff_kind(b""), "empty");
        assert_eq!(sniff_kind(b"hello"), "unknown");
    }
}

fn sha1_hex(data: &[u8]) -> String {
    use sha1::Digest as _;
    let mut h = sha1::Sha1::new();
    h.update(data);
    hex_of(h.finalize())
}

fn md5_hex(data: &[u8]) -> String {
    format!("{:x}", md5::compute(data))
}

// 为已落盘的产物写 .md5 / .sha1（中央仓要求每个文件都有校验和）
fn write_checksums(path: &Path) -> Result<(PathBuf, PathBuf), String> {
    let data = fs::read(path).map_err(|e| format!("计算校验和失败，读文件出错：{e}"))?;
    let stem = path.file_name().and_then(|s| s.to_str()).ok_or_else(|| "计算校验和失败：文件名无效".to_string())?;
    let m5 = path.with_file_name(format!("{stem}.md5"));
    let s1 = path.with_file_name(format!("{stem}.sha1"));
    fs::write(&m5, md5_hex(&data)).map_err(|e| format!("写 .md5 失败：{e}"))?;
    fs::write(&s1, sha1_hex(&data)).map_err(|e| format!("写 .sha1 失败：{e}"))?;
    Ok((m5, s1))
}

struct StagedArtifact {
    rel_files: Vec<String>,
    main_file_name: String,
    size: u64,
    sha256: String,
}

fn stage_one(
    staging: &Path,
    group_id: &str,
    artifact_id: &str,
    version: &str,
    packaging: &str,
    classifier: &str,
    meta: &PomMeta,
    source_file: &Path,
    skip_sign: bool,
) -> Result<StagedArtifact, String> {
    let ext = extension_for(packaging, source_file);
    let main_name = artifact_file_name(artifact_id, version, classifier, &ext);
    let pom_name = format!("{artifact_id}-{version}.pom");
    let sources_name = format!("{artifact_id}-{version}-sources.jar");
    let javadoc_name = format!("{artifact_id}-{version}-javadoc.jar");

    let dest_dir = staging
        .join(maven_dir(group_id))
        .join(artifact_id)
        .join(version);
    fs::create_dir_all(&dest_dir).map_err(|e| format!("创建暂存目录失败：{e}"))?;

    let data = fs::read(source_file).map_err(|e| format!("读取待发布文件失败：{e}"))?;
    let size = data.len() as u64;
    let mut h = Sha256::new();
    h.update(&data);
    let sha256 = hex_of(h.finalize());
    validate_artifact_bytes(&ext, &data, source_file)?;

    fs::write(dest_dir.join(&main_name), &data).map_err(|e| format!("写入主构件失败：{e}"))?;
    let pom = build_minimal_pom(group_id, artifact_id, version, packaging, meta);
    fs::write(dest_dir.join(&pom_name), pom.as_bytes()).map_err(|e| format!("写入 pom 失败：{e}"))?;
    build_empty_jar(&dest_dir.join(&sources_name), artifact_id, version, "sources")?;
    build_empty_jar(&dest_dir.join(&javadoc_name), artifact_id, version, "javadoc")?;

    let mut rel_files = vec![
        maven_rel_path(group_id, artifact_id, version, &main_name),
        maven_rel_path(group_id, artifact_id, version, &pom_name),
        maven_rel_path(group_id, artifact_id, version, &sources_name),
        maven_rel_path(group_id, artifact_id, version, &javadoc_name),
    ];

    if !skip_sign {
        for name in [main_name.clone(), pom_name.clone(), sources_name.clone(), javadoc_name.clone()] {
            let p = dest_dir.join(&name);
            sign_file(&p)?;
            rel_files.push(maven_rel_path(
                group_id,
                artifact_id,
                version,
                &format!("{name}.asc"),
            ));
        }
    }
    // 中央仓要求每个文件都有 .md5 / .sha1，先列出已有文件再逐个生成
    let mut existing: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(&dest_dir).map_err(|e| format!("列举暂存文件失败：{e}"))? {
        let entry = entry.map_err(|e| format!("列举暂存文件失败：{e}"))?;
        let q = entry.path();
        if q.is_file() {
            existing.push(q);
        }
    }
    for q in &existing {
        let (m5, s1) = write_checksums(q)?;
        for cp in [&m5, &s1] {
            if let Some(n) = cp.file_name().and_then(|s| s.to_str()) {
                rel_files.push(maven_rel_path(group_id, artifact_id, version, n));
            }
        }
    }
    Ok(StagedArtifact { rel_files, main_file_name: main_name, size, sha256 })
}

fn zip_staging(staging: &Path, bundle_path: &Path) -> Result<(), String> {
    let f = fs::File::create(bundle_path).map_err(|e| format!("创建 bundle 失败：{e}"))?;
    let mut zip = ZipWriter::new(f);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut stack = vec![staging.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(|e| format!("遍历暂存失败：{e}"))? {
            let entry = entry.map_err(|e| format!("遍历暂存失败：{e}"))?;
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(staging).map_err(|e| format!("路径处理失败：{e}"))?;
                let name = rel.to_string_lossy().replace('\\', "/");
                zip.start_file(name, opts).map_err(|e| format!("写入 bundle 失败：{e}"))?;
                let data = fs::read(&p).map_err(|e| format!("读取暂存失败：{e}"))?;
                zip.write_all(&data).map_err(|e| format!("写入 bundle 失败：{e}"))?;
            }
        }
    }
    zip.finish().map_err(|e| format!("完成 bundle 失败：{e}"))?;
    Ok(())
}

// ---------- 本地存储 ----------
fn app_dir(app: &AppHandle) -> PathBuf {
    let base = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("mavenshare"));
    let _ = fs::create_dir_all(&base);
    base
}

fn history_path(app: &AppHandle) -> PathBuf {
    app_dir(app).join("history.json")
}
fn catalog_path(app: &AppHandle, group_id: &str) -> PathBuf {
    let safe: String = group_id.chars().map(|c| if c == '.' || c == '-' || c == '_' || c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    app_dir(app).join(format!("catalog-{safe}.json"))
}
fn config_path(app: &AppHandle) -> PathBuf {
    app_dir(app).join("config.json")
}

fn load_history(app: &AppHandle) -> Vec<HistoryEntry> {
    fs::read_to_string(history_path(app))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}
fn save_history(app: &AppHandle, h: &[HistoryEntry]) {
    let _ = fs::write(history_path(app), serde_json::to_string_pretty(h).unwrap_or_default());
}
fn load_catalog(app: &AppHandle, group_id: &str) -> CatalogFile {
    fs::read_to_string(catalog_path(app, group_id))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(CatalogFile { group_id: group_id.to_string(), artifact_id: FIXED_CATALOG_ARTIFACT.to_string(), updated_at: Utc::now().to_rfc3339(), latest_version: None, entries: vec![] })
}
fn save_catalog(app: &AppHandle, c: &CatalogFile) {
    let _ = fs::write(catalog_path(app, &c.group_id), serde_json::to_string_pretty(c).unwrap_or_default());
}

fn store_token(username: &str, token: &str) {
    if username.is_empty() || token.is_empty() {
        return;
    }
    if let Ok(entry) = keyring::Entry::new("MavenShare", username) {
        let _ = entry.set_password(token);
    }
}
fn load_token(username: &str) -> Option<String> {
    if username.is_empty() {
        return None;
    }
    keyring::Entry::new("MavenShare", username).ok()?.get_password().ok()
}
fn token_b64(token: &str) -> String {
    B64.encode(token.as_bytes())
}

// ---------- Portal API ----------
async fn portal_upload(bundle: &Path, username: &str, token: &str) -> Result<String, String> {
    let data = fs::read(bundle).map_err(|e| format!("读取 bundle 失败：{e}"))?;
    let file_name = bundle.file_name().and_then(|s| s.to_str()).unwrap_or("bundle.zip").to_string();
    let part = reqwest::multipart::Part::bytes(data).file_name(file_name);
    let form = reqwest::multipart::Form::new().part("bundle", part);
    let client = portal_client();
    let resp = client
        .post(PORTAL_UPLOAD_URL)
        .header("Authorization", format!("Bearer {}", bearer_of(username, token)))
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("上传 bundle 失败（网络）：{e}"))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let snippet: String = body.chars().take(500).collect();
        return Err(format!("Portal 上传失败 HTTP {status}：{snippet}"));
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
        for key in ["deploymentId", "deployment_id", "id"] {
            if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
                if !s.trim().is_empty() {
                    return Ok(s.trim().to_string());
                }
            }
        }
    }
    let t = body.trim().to_string();
    if t.is_empty() || t.len() > 200 {
        return Err(format!("上传成功但未取到 deploymentId，返回：{}", body.chars().take(300).collect::<String>()));
    }
    Ok(t)
}

async fn portal_status_once(deployment_id: &str, username: &str, token: &str) -> Result<String, String> {
    let client = portal_client();
    let url = format!("{PORTAL_STATUS_URL}?id={deployment_id}");
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", bearer_of(username, token)))
        .send()
        .await
        .map_err(|e| format!("查询状态失败（网络）：{e}"))?;
    let body = resp.text().await.unwrap_or_default();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
        for key in ["deploymentState", "state", "status"] {
            if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
                return Ok(s.to_string());
            }
        }
        return Ok(body.chars().take(300).collect());
    }
    Ok(body.chars().take(300).collect())
}

async fn portal_poll(deployment_id: &str, username: &str, token: &str, max_tries: u32) -> String {
    for _ in 0..max_tries {
        match portal_status_once(deployment_id, username, token).await {
            Ok(s) => {
                let upper = s.to_uppercase();
                if upper.contains("PUBLISHED") || upper.contains("FAILED") || upper.contains("VALIDAT") {
                    if upper.contains("PUBLISHED") || upper.contains("VALIDATED") {
                        return s;
                    }
                    if upper.contains("FAILED") {
                        return s;
                    }
                }
                if upper.contains("PUBLISHED") {
                    return s;
                }
            }
            Err(e) => {
                return format!("STATUS_QUERY_FAILED: {e}");
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(8)).await;
    }
    "POLL_TIMEOUT（已上传，请稍后在 Portal 页面确认状态）".to_string()
}

// ---------- Tauri 命令 ----------
#[tauri::command]
fn cmd_validate(group_id: String, artifact_id: String, version: String, license_name: String, license_url: String, scm_url: String, dev_id: String, dev_name: String, dev_email: String) -> Result<Vec<String>, String> {
    validate_gav(&group_id, &artifact_id, &version)?;
    let tmp = PublishRequest {
        file_path: String::new(),
        group_id: String::new(),
        artifact_id: String::new(),
        version: String::new(),
        packaging: String::new(),
        classifier: String::new(),
        portal_username: String::new(),
        portal_token: String::new(),
        publish_catalog: true,
        license_name,
        license_url,
        scm_url,
        dev_id,
        dev_name,
        dev_email,
    };
    let raw_license_empty = tmp.license_name.trim().is_empty();
    let _meta = validate_pom_meta(&tmp)?;
    let mut w = gav_warnings(&group_id, &version);
    if raw_license_empty {
        w.push("License 未填写，已默认 MIT License；如项目使用其他协议请在表单 License 处修改。".to_string());
    }
    Ok(w)
}

#[tauri::command]
fn cmd_env(app: AppHandle) -> EnvInfo {
    let (available, ver) = match gpg_version() {
        Some(v) => (true, v),
        None => (false, "未检测到 gpg，请用下方“0 · GPG 安装与密钥”一键安装".to_string()),
    };
    EnvInfo { gpg_available: available, gpg_version: ver, app_data_dir: app_dir(&app).to_string_lossy().to_string() }
}

#[tauri::command]
fn cmd_file_info(file_path: String) -> Result<serde_json::Value, String> {
    let p = Path::new(&file_path);
    if !p.exists() {
        return Err("所选文件不存在，请重新选择。".to_string());
    }
    let meta = fs::metadata(p).map_err(|e| format!("读取文件信息失败：{e}"))?;
    let sha256 = sha256_file(p)?;
    let kind = sniff_file_head(p);
    let kind_desc = kind_desc_of(kind);
    Ok(serde_json::json!({
        "file_name": p.file_name().and_then(|s| s.to_str()).unwrap_or(""),
        "size": meta.len(),
        "sha256": sha256,
        "kind": kind,
        "kind_desc": kind_desc,
    }))
}

#[tauri::command]
fn cmd_dry_run(app: AppHandle, request: PublishRequest) -> Result<DryRunResult, String> {
    validate_gav(&request.group_id, &request.artifact_id, &request.version)?;
    let meta = validate_pom_meta(&request)?;
    let src = Path::new(&request.file_path);
    if !src.exists() {
        return Err("待发布文件不存在，请重新选择。".to_string());
    }
    let packaging = if request.packaging.trim().is_empty() { "jar".to_string() } else { request.packaging.clone() };
    let work = app_dir(&app).join("bundles");
    let _ = fs::create_dir_all(&work);
    let staging = work.join(format!("stage-{}-{}", request.artifact_id, chrono::Utc::now().timestamp_millis()));
    fs::create_dir_all(&staging).map_err(|e| format!("创建暂存失败：{e}"))?;
    let staged = stage_one(&staging, &request.group_id, &request.artifact_id, &request.version, &packaging, &request.classifier, &meta, src, true)?;
    let bundle_path = work.join(format!("{}-{}-bundle.zip", request.artifact_id, request.version));
    zip_staging(&staging, &bundle_path)?;
    let pom = build_minimal_pom(&request.group_id, &request.artifact_id, &request.version, &packaging, &meta);
    let mut files = staged.rel_files.clone();
    files.push("（注：正式发布时每个文件还会生成 .asc 签名，需本机有 gpg）".to_string());
    let _ = fs::remove_dir_all(&staging);
    Ok(DryRunResult {
        bundle_path: bundle_path.to_string_lossy().to_string(),
        files,
        sha256: staged.sha256,
        size: staged.size,
        pom_preview: pom.chars().take(800).collect(),
    })
}

#[tauri::command]
async fn cmd_publish(app: AppHandle, request: PublishRequest) -> Result<PublishResult, String> {
    let _pub_meta = validate_pom_meta(&request)?;
    let portal_username = request.portal_username.trim().to_string();
    let portal_token = request.portal_token.trim().to_string();
    validate_gav(&request.group_id, &request.artifact_id, &request.version)?;
    let src = PathBuf::from(&request.file_path);
    if !src.exists() {
        return Err("待发布文件不存在，请重新选择。".to_string());
    }
    if request.portal_username.trim().is_empty() || request.portal_token.trim().is_empty() {
        let u = if request.portal_username.trim().is_empty() { "用户名为空" } else { "用户名已收到" };
        let t = if request.portal_token.trim().is_empty() { "Token 为空" } else { "Token 已收到" };
        return Err(format!("请先填写 Central Portal 用户名与 Token（{u}，{t}，在 central.sonatype.com 的 User Token 页面生成）。"));
    }
    if gpg_version().is_none() {
        return Err("未检测到 gpg：正式版必须签名，请先在“0 · GPG 安装与密钥”一键安装并生成密钥。".to_string());
    }
    let packaging = if request.packaging.trim().is_empty() { "jar".to_string() } else { request.packaging.clone() };
    let work = app_dir(&app).join("bundles");
    let _ = fs::create_dir_all(&work);

    let staging = work.join(format!("stage-{}-{}", request.artifact_id, chrono::Utc::now().timestamp_millis()));
    fs::create_dir_all(&staging).map_err(|e| format!("创建暂存失败：{e}"))?;
    let staged = stage_one(&staging, &request.group_id, &request.artifact_id, &request.version, &packaging, &request.classifier, &_pub_meta, &src, false)?;
    let bundle_path = work.join(format!("{}-{}-{}-bundle.zip", request.artifact_id, request.version, chrono::Utc::now().timestamp_millis()));
    zip_staging(&staging, &bundle_path)?;
    let _ = fs::remove_dir_all(&staging);

    let file_name_only: String = src.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let deployment_id = portal_upload(&bundle_path, &portal_username, &portal_token).await?;
    let status = portal_poll(&deployment_id, &portal_username, &portal_token, 36).await;
    let now = Utc::now().to_rfc3339();

    let mut history = load_history(&app);
    history.push(HistoryEntry {
        kind: "artifact".to_string(),
        group_id: request.group_id.clone(),
        artifact_id: request.artifact_id.clone(),
        version: request.version.clone(),
        file_name: if file_name_only.is_empty() { staged.main_file_name.clone() } else { file_name_only.clone() },
        size: staged.size,
        sha256: staged.sha256.clone(),
        published_at: now.clone(),
        deployment_id: deployment_id.clone(),
        status: status.clone(),
    });
    save_history(&app, &history);
    store_token(&portal_username, &portal_token);
    let _ = fs::write(config_path(&app), serde_json::json!({"portal_username": portal_username.clone(), "token_b64_hint": token_b64(&portal_token)[..8.min(portal_token.len())].to_string()}).to_string());

    let (catalog_version, catalog_status) = if request.publish_catalog {
        match publish_catalog_internal(&app, &request, &file_name_only, staged.size, &staged.sha256, &now, &deployment_id).await {
            Ok((v, s)) => (Some(v), Some(s)),
            Err(e) => (None, Some(format!("清单发布失败：{e}（主构件不受影响）"))),
        }
    } else {
        (None, None)
    };

    Ok(PublishResult { deployment_id, status, catalog_version, catalog_status })
}

async fn publish_catalog_internal(
    app: &AppHandle,
    request: &PublishRequest,
    file_name: &str,
    size: u64,
    sha256: &str,
    now: &str,
    deployment_id: &str,
) -> Result<(String, String), String> {
    let portal_username = request.portal_username.trim().to_string();
    let portal_token = request.portal_token.trim().to_string();
    let mut catalog = load_catalog(app, &request.group_id);
    catalog.group_id = request.group_id.clone();
    catalog.artifact_id = FIXED_CATALOG_ARTIFACT.to_string();
    catalog.entries.push(CatalogEntry {
        group_id: request.group_id.clone(),
        artifact_id: request.artifact_id.clone(),
        version: request.version.clone(),
        file_name: if file_name.is_empty() { format!("{}-{}", request.artifact_id, request.version) } else { file_name.to_string() },
        packaging: request.packaging.clone(),
        classifier: request.classifier.clone(),
        size,
        sha256: sha256.to_string(),
        published_at: now.to_string(),
        deployment_id: deployment_id.to_string(),
    });
    let next_ver = next_catalog_version(catalog.latest_version.as_deref());
    catalog.latest_version = Some(next_ver.clone());
    catalog.updated_at = now.to_string();
    save_catalog(app, &catalog);

    let work = app_dir(app).join("bundles");
    let _ = fs::create_dir_all(&work);
    let tmp_json = work.join(format!("catalog-tmp-{}.json", chrono::Utc::now().timestamp_millis()));
    let payload = serde_json::json!({
        "groupId": catalog.group_id,
        "artifactId": catalog.artifact_id,
        "version": next_ver,
        "updatedAt": catalog.updated_at,
        "latestVersion": next_ver,
        "entries": catalog.entries,
    });
    fs::write(&tmp_json, serde_json::to_string_pretty(&payload).unwrap_or_default())
        .map_err(|e| format!("写入清单暂存失败：{e}"))?;

    let staging = work.join(format!("stage-catalog-{}", chrono::Utc::now().timestamp_millis()));
    fs::create_dir_all(&staging).map_err(|e| format!("创建清单暂存失败：{e}"))?;
    let staged = stage_one(&staging, &request.group_id, FIXED_CATALOG_ARTIFACT, &next_ver, "json", "", &PomMeta::from_request(&request), &tmp_json, false)?;
    let _ = staged;
    let bundle_path = work.join(format!("catalog-{next_ver}-{}-bundle.zip", chrono::Utc::now().timestamp_millis()));
    zip_staging(&staging, &bundle_path)?;
    let _ = fs::remove_dir_all(&staging);
    let _ = fs::remove_file(&tmp_json);

    let dep = portal_upload(&bundle_path, &portal_username, &portal_token).await?;
    let st = portal_poll(&dep, &portal_username, &portal_token, 22).await;
    let mut history = load_history(app);
    history.push(HistoryEntry {
        kind: "catalog".to_string(),
        group_id: request.group_id.clone(),
        artifact_id: FIXED_CATALOG_ARTIFACT.to_string(),
        version: next_ver.clone(),
        file_name: "catalog.json".to_string(),
        size: 0,
        sha256: "".to_string(),
        published_at: now.to_string(),
        deployment_id: dep.clone(),
        status: st.clone(),
    });
    save_history(app, &history);
    Ok((next_ver.clone(), format!("清单 {next_ver}：{st}（deployment {dep}）")))
}

#[tauri::command]
fn cmd_history(app: AppHandle) -> Vec<HistoryEntry> {
    let mut h = load_history(&app);
    h.reverse();
    h
}

#[tauri::command]
fn cmd_catalog(app: AppHandle, group_id: String) -> serde_json::Value {
    let c = load_catalog(&app, &group_id);
    serde_json::json!({
        "group_id": c.group_id,
        "artifact_id": c.artifact_id,
        "updated_at": c.updated_at,
        "latest_version": c.latest_version,
        "entries": c.entries,
    })
}

#[tauri::command]
fn cmd_save_token(portal_username: String, portal_token: String) -> Result<String, String> {
    if portal_username.trim().is_empty() {
        return Err("用户名不能为空".to_string());
    }
    store_token(&portal_username, &portal_token);
    Ok("Token 已保存到本机凭证库（Windows 凭据管理器）".to_string())
}

#[tauri::command]
fn cmd_load_config(app: AppHandle) -> serde_json::Value {
    let username: String = fs::read_to_string(config_path(&app))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("portal_username").and_then(|x| x.as_str()).map(|s| s.to_string()))
        .unwrap_or_default();
    let has_token = if username.is_empty() { false } else { load_token(&username).is_some() };
    serde_json::json!({"portal_username": username, "has_token": has_token})
}

#[derive(Debug, Serialize)]
struct GpgKeyInfo {
    fingerprint: String,
    user_id: String,
}

fn parse_colon_keys(out: &str) -> Vec<GpgKeyInfo> {
    let mut keys = Vec::new();
    let mut cur_fpr = String::new();
    let mut cur_uid = String::new();
    let push_cur = |keys: &mut Vec<GpgKeyInfo>, fpr: &mut String, uid: &mut String| {
        if !fpr.is_empty() {
            keys.push(GpgKeyInfo { fingerprint: std::mem::take(fpr), user_id: std::mem::take(uid) });
        }
    };
    for line in out.lines() {
        let kind = line.split(':').next().unwrap_or("");
        match kind {
            "sec" => {
                push_cur(&mut keys, &mut cur_fpr, &mut cur_uid);
            }
            "fpr" => {
                let f: Vec<&str> = line.split(':').collect();
                if f.len() > 9 && !f[9].is_empty() && cur_fpr.is_empty() {
                    cur_fpr = f[9].to_string();
                }
            }
            "uid" => {
                let f: Vec<&str> = line.split(':').collect();
                if f.len() > 9 && !f[9].is_empty() && cur_uid.is_empty() {
                    cur_uid = f[9].replace("\\x3a", ":");
                }
            }
            _ => {}
        }
    }
    push_cur(&mut keys, &mut cur_fpr, &mut cur_uid);
    keys
}

fn gpg_bin_or_err() -> Result<String, String> {
    for bin in gpg_candidates() {
        if Command::new(bin).arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
            return Ok(bin.to_string());
        }
    }
    Err("未检测到 gpg：请先在下方“一键安装 Gpg4win”，或去官网 gpg4win.org 手动安装。".to_string())
}

#[tauri::command]
fn cmd_gpg_keys() -> Result<Vec<GpgKeyInfo>, String> {
    let bin = gpg_bin_or_err()?;
    let out = Command::new(&bin)
        .args(["--list-secret-keys", "--with-colons", "--fingerprint"])
        .output()
        .map_err(|e| format!("查询密钥失败：{e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    Ok(parse_colon_keys(&text))
}

#[tauri::command]
fn cmd_gpg_gen_key(name: String, email: String, passphrase: String) -> Result<String, String> {
    let bin = gpg_bin_or_err()?;
    let name = name.trim();
    let email = email.trim();
    if name.is_empty() {
        return Err("生成密钥需要填写姓名（任意昵称即可）。".to_string());
    }
    if !email.contains('@') {
        return Err("请填写有效的邮箱地址，密钥标识需要它。".to_string());
    }
    let uid = format!("{name} <{email}>");
    let out = Command::new(&bin)
        .args(["--batch", "--pinentry-mode", "loopback", "--passphrase", &passphrase,
               "--quick-generate-key", &uid, "rsa3072", "sign", "never"])
        .output()
        .map_err(|e| format!("生成密钥失败：{e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        let snippet: String = err.chars().take(400).collect();
        return Err(format!("生成密钥失败：{snippet}"));
    }
    let out2 = Command::new(&bin)
        .args(["--list-keys", "--with-colons", email])
        .output()
        .map_err(|e| format!("密钥已生成，但查询指纹失败：{e}"))?;
    let text = String::from_utf8_lossy(&out2.stdout).to_string();
    let keys = parse_colon_keys(&text);
    Ok(keys.into_iter().next().map(|k| k.fingerprint).unwrap_or_else(|| "已生成（指纹为空，请点刷新密钥列表确认）".to_string()))
}

#[tauri::command]
async fn cmd_gpg_publish_key() -> Result<String, String> {
    let bin = gpg_bin_or_err()?;
    let keys = cmd_gpg_keys()?;
    if keys.is_empty() {
        return Err("没有可发布的 GPG 密钥：请先在“0 · GPG”里生成密钥后再试。".to_string());
    }
    let fps: Vec<String> = keys.into_iter().map(|k| k.fingerprint).collect();
    let joined = tokio::task::spawn_blocking(move || {
        let mut parts = Vec::new();
        for fp in &fps {
            for server in ["hkps://keyserver.ubuntu.com", "hkps://keys.openpgp.org"] {
                let out = Command::new(&bin).args(["--keyserver", server, "--send-keys", fp]).output();
                match out {
                    Ok(o) if o.status.success() => parts.push(format!("{fp} -> {server} 成功")),
                    Ok(o) => {
                        let e = String::from_utf8_lossy(&o.stderr);
                        let s: String = e.chars().take(200).collect();
                        parts.push(format!("{fp} -> {server} 失败：{s}"));
                    }
                    Err(e) => parts.push(format!("{fp} -> {server} 失败：{e}")),
                }
            }
        }
        parts.join("\n")
    })
    .await
    .map_err(|e| format!("发布公钥任务失败：{e}"))?;
    Ok(format!("公钥发布结果：\n{joined}\n说明：keyserver 全网同步通常要几分钟到几小时，可在 https://keyserver.ubuntu.com 按指纹搜索确认；keys.openpgp.org 还需要到邮箱点验证链接才会公开。同步完成前发布仍会报 Invalid signature，稍后再试。"))
}

#[tauri::command]
async fn cmd_gpg_install() -> Result<String, String> {
    if let Some(v) = gpg_version() {
        return Ok(format!("已检测到 {v}，无需安装。"));
    }
    let has_winget = Command::new("winget").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if !has_winget {
        return Err("本机没有 winget（需 Win10 1709+ 自带的 App Installer），请点“打开官网下载页”去 gpg4win.org 手动下载安装。".to_string());
    }
    let output = tokio::task::spawn_blocking(|| {
        Command::new("winget")
            .args(["install", "--id", "GnuPG.Gpg4win", "-e", "--silent",
                   "--accept-package-agreements", "--accept-source-agreements"])
            .output()
    })
    .await
    .map_err(|e| format!("安装任务失败：{e}"))?
    .map_err(|e| format!("启动 winget 失败：{e}"))?;
    if !output.status.success() {
        let combined = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        let tail: String = combined.chars().rev().take(600).collect::<String>().chars().rev().collect();
        return Err(format!("winget 安装失败（若弹出系统权限确认请点“是”后重试）：{tail}"));
    }
    match gpg_version() {
        Some(v) => Ok(format!("安装成功：{v}。下一步请在下方填写姓名邮箱并生成密钥。")),
        None => Ok("安装程序已执行，但当前进程 PATH 中暂未找到 gpg（刚装完常见）。请点“重新检测”；若仍没有请重启本应用，Gpg4win 默认装在 C:\\Program Files (x86)\\GnuPG。".to_string()),
    }
}
#[cfg_attr(mobile, tauri::mobile_entry_point)]
const APP_USER_AGENT: &str = "MavenShare/0.1.0";

fn portal_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(APP_USER_AGENT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

fn bearer_of(username: &str, token: &str) -> String {
    B64.encode(format!("{username}:{token}"))
}

fn interpret_check_status(code: u16) -> Result<String, String> {
    match code {
        401 | 403 => Err(format!("凭证无效：Portal 返回 HTTP {code}。请确认用的是 Account 页 Generate User Token 生成的用户名与密码（不是登录密码），必要时重新生成后再试。")),
        _ => Ok(format!("凭证有效：网关已接受该 Token（检查调用返回 HTTP {code}）。")),
    }
}

#[tauri::command]
async fn cmd_portal_check(portal_username: String, portal_token: String) -> Result<String, String> {
    let portal_username = portal_username.trim().to_string();
    let portal_token = portal_token.trim().to_string();
    if portal_username.trim().is_empty() || portal_token.trim().is_empty() {
        return Err("请先填写 Portal 用户名与 Token。".to_string());
    }
    let client = portal_client();
    let url = format!("{PORTAL_STATUS_URL}?id=00000000-0000-0000-0000-000000000000");
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", bearer_of(&portal_username, &portal_token)))
        .send()
        .await
        .map_err(|e| format!("连接 Portal 失败（网络）：{e}"))?;
    interpret_check_status(resp.status().as_u16())
}
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            cmd_validate,
            cmd_env,
            cmd_file_info,
            cmd_dry_run,
            cmd_publish,
            cmd_history,
            cmd_catalog,
            cmd_save_token,
            cmd_load_config,
            cmd_portal_check,
            cmd_gpg_keys,
            cmd_gpg_gen_key,
            cmd_gpg_install,
            cmd_gpg_publish_key
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gav_ok() {
        assert!(validate_gav("com.example", "my-lib", "1.0.0").is_ok());
        assert!(validate_gav("com.example", "my-lib", "1.0.0-SNAPSHOT").is_ok());
    }
    #[test]
    fn gav_rejects() {
        assert!(validate_gav("", "a", "1.0").is_err());
        assert!(validate_gav("com example", "a", "1.0").is_err());
        assert!(validate_gav("com.example", "", "1.0").is_err());
        assert!(validate_gav("com.example", "a", "latest").is_err());
    }
    #[test]
    fn warnings_cover_snapshot_and_release() {
        let w = gav_warnings("com.example", "1.0.0");
        assert!(w.iter().any(|s| s.contains("GPG")));
        let w2 = gav_warnings("mygroup", "2.0-SNAPSHOT");
        assert!(w2.iter().any(|s| s.contains("点号")));
    }
    #[allow(dead_code)]
    fn test_meta() -> PomMeta {
        PomMeta {
            license_name: "MIT".to_string(),
            license_url: "https://opensource.org/licenses/MIT".to_string(),
            scm_url: "https://github.com/example/demo".to_string(),
            dev_id: "dev".to_string(),
            dev_name: "Dev".to_string(),
            dev_email: "dev@example.com".to_string(),
        }
    }
    fn test_zip_bytes() -> Vec<u8> {
        let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        w.start_file("hello.txt", SimpleFileOptions::default()).unwrap();
        w.write_all(b"hello-mavenshare").unwrap();
        w.finish().unwrap().into_inner()
    }
    fn test_request() -> PublishRequest {
        PublishRequest {
            file_path: String::new(),
            group_id: "com.example".to_string(),
            artifact_id: "demo".to_string(),
            version: "1.0.0".to_string(),
            packaging: "jar".to_string(),
            classifier: String::new(),
            portal_username: String::new(),
            portal_token: String::new(),
            publish_catalog: true,
            license_name: "MIT".to_string(),
            license_url: String::new(),
            scm_url: "https://github.com/example/demo".to_string(),
            dev_id: String::new(),
            dev_name: "Dev".to_string(),
            dev_email: "dev@example.com".to_string(),
        }
    }
    #[test]
    #[test]
    fn pom_contains_gav() {
        let p = build_minimal_pom("com.example", "demo", "1.2.3", "jar", &test_meta());
        assert!(p.contains("<groupId>com.example</groupId>"));
        assert!(p.contains("<artifactId>demo</artifactId>"));
        assert!(p.contains("<version>1.2.3</version>"));
        assert!(p.contains("<licenses>"));
        assert!(p.contains("https://opensource.org/licenses/MIT"));
        assert!(p.contains("<scm>"));
        assert!(p.contains("https://github.com/example/demo"));
        assert!(p.contains("<developers>"));
        assert!(p.contains("dev@example.com"));
    }
    #[test]
    fn pom_escapes_xml() {
        let mut m = test_meta();
        m.scm_url = "https://example.com/a?b=1&c=2".to_string();
        let p = build_minimal_pom("g", "a", "1", "jar", &m);
        assert!(p.contains("b=1&amp;c=2"));
        assert!(!p.contains("b=1&c=2<"));
    }
    #[test]
    fn license_url_table() {
        assert_eq!(license_url_for("MIT"), "https://opensource.org/licenses/MIT");
        assert_eq!(license_url_for("Apache-2.0"), "https://www.apache.org/licenses/LICENSE-2.0");
        assert_eq!(license_url_for("whatever"), "https://opensource.org/licenses/MIT");
        assert_eq!(license_display("BSD-3-Clause"), "BSD 3-Clause License");
    }
    #[test]
    fn meta_validation_rules() {
        let mut r = test_request();
        let m = validate_pom_meta(&r).unwrap();
        assert_eq!(m.license_name, "MIT");
        assert_eq!(m.license_url, "https://opensource.org/licenses/MIT");
        assert_eq!(m.dev_id, "Dev");
        r.scm_url = String::new();
        assert!(validate_pom_meta(&r).unwrap_err().contains("SCM"));
        r.scm_url = "ftp://x/y".to_string();
        assert!(validate_pom_meta(&r).is_err());
        r.scm_url = "https://github.com/example/demo".to_string();
        r.dev_email = "bad".to_string();
        assert!(validate_pom_meta(&r).is_err());
        r.dev_email = "dev@example.com".to_string();
        r.dev_name = String::new();
        assert!(validate_pom_meta(&r).is_err());
    }
    #[test]
    fn checksums_written_and_match() {
        let base = std::env::temp_dir().join(format!("mavenshare-test-sum-{}", chrono::Utc::now().timestamp_millis()));
        let _ = fs::create_dir_all(&base);
        let input = base.join("input.zip");
        fs::write(&input, test_zip_bytes()).unwrap();
        let staging = base.join("staging");
        fs::create_dir_all(&staging).unwrap();
        let staged = stage_one(&staging, "com.example", "demo", "1.0.0", "jar", "", &test_meta(), &input, true).unwrap();
        // 4 artifacts + 8 checksums, no .asc in dry-run mode
        assert_eq!(staged.rel_files.len(), 12);
        let dir = staging.join("com/example/demo/1.0.0");
        let jar = fs::read(dir.join("demo-1.0.0.jar")).unwrap();
        assert_eq!(fs::read_to_string(dir.join("demo-1.0.0.jar.md5")).unwrap(), md5_hex(&jar));
        assert_eq!(fs::read_to_string(dir.join("demo-1.0.0.jar.sha1")).unwrap(), sha1_hex(&jar));
        let pom = fs::read(dir.join("demo-1.0.0.pom")).unwrap();
        assert_eq!(fs::read_to_string(dir.join("demo-1.0.0.pom.md5")).unwrap(), md5_hex(&pom));
        let bundle = base.join("b.zip");
        zip_staging(&staging, &bundle).unwrap();
        let f = fs::File::open(&bundle).unwrap();
        let ar = zip::ZipArchive::new(f).unwrap();
        assert!(ar.len() >= 12);
        let _ = fs::remove_dir_all(&base);
    }
    #[test]
    fn catalog_version_increments() {
        assert_eq!(next_catalog_version(None), "1.0.0");
        assert_eq!(next_catalog_version(Some("1.0.0")), "1.0.1");
        assert_eq!(next_catalog_version(Some("1.0.9")), "1.0.10");
        assert_eq!(next_catalog_version(Some("abc")), "abc.1");
    }
    #[test]
    fn portal_client_builds() { let _ = portal_client(); }
    #[test]
    fn bearer_and_check_interpret() {
        assert_eq!(bearer_of("a", "b"), "YTpi");
        assert!(interpret_check_status(404).is_ok());
        assert!(interpret_check_status(400).is_ok());
        assert!(interpret_check_status(401).is_err());
        assert!(interpret_check_status(403).is_err());
    }
    #[test]
    fn colon_keys_parse() {
        let sample = "sec:u:3072:1:ABCDEF01:1700000000:::u:::scESC::::::23::0:\nfpr:::::::::AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA:\nuid:u::::1700000000::HASH::Test User <test@example.com>::::::::::0:\n";
        let keys = parse_colon_keys(sample);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].fingerprint, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        assert!(keys[0].user_id.contains("test@example.com"));
        assert!(parse_colon_keys("").is_empty());
    }
    #[test]
    fn maven_path_layout() {
        let rel = maven_rel_path("com.example", "demo", "1.0", "demo-1.0.jar");
        assert_eq!(rel, "com/example/demo/1.0/demo-1.0.jar");
        let n = artifact_file_name("demo", "1.0", "", "jar");
        assert_eq!(n, "demo-1.0.jar");
        let n2 = artifact_file_name("demo", "1.0", "linux", "jar");
        assert_eq!(n2, "demo-1.0-linux.jar");
    }
    #[test]
    fn bundle_staging_structure() {
        let base = std::env::temp_dir().join(format!("mavenshare-test-{}", chrono::Utc::now().timestamp_millis()));
        let _ = fs::create_dir_all(&base);
        let input = base.join("input.zip");
        fs::write(&input, test_zip_bytes()).unwrap();
        let staging = base.join("staging");
        fs::create_dir_all(&staging).unwrap();
        let staged = stage_one(&staging, "com.example", "demo", "1.0.0", "jar", "", &test_meta(), &input, true).unwrap();
        assert_eq!(staged.rel_files.len(), 12);
        assert!(staging.join("com/example/demo/1.0.0/demo-1.0.0.jar").exists());
        assert!(staging.join("com/example/demo/1.0.0/demo-1.0.0.pom").exists());
        assert!(staging.join("com/example/demo/1.0.0/demo-1.0.0-sources.jar").exists());
        assert!(staging.join("com/example/demo/1.0.0/demo-1.0.0-javadoc.jar").exists());
        let bundle = base.join("b.zip");
        zip_staging(&staging, &bundle).unwrap();
        let f = fs::File::open(&bundle).unwrap();
        let ar = zip::ZipArchive::new(f).unwrap();
        assert!(ar.len() >= 12);
        let _ = fs::remove_dir_all(&base);
    }
}
