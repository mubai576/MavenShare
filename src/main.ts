import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";

const $ = (id: string) => document.getElementById(id) as HTMLElement;
const input = (id: string) => document.getElementById(id) as HTMLInputElement;
const sel = (id: string) => document.getElementById(id) as HTMLSelectElement;

let pickedFile = "";

// ===== 1 section form memory: last values + history dropdowns (localStorage, token excluded) =====
const MEM_KEY = "mavenshare:form-memory:v1";
const MEM_MAX = 20;
const LICENSE_URLS: Record<string, string> = {
  "MIT": "https://opensource.org/licenses/MIT",
  "Apache-2.0": "https://www.apache.org/licenses/LICENSE-2.0",
  "BSD-3-Clause": "https://opensource.org/licenses/BSD-3-Clause",
  "GPL-3.0-only": "https://www.gnu.org/licenses/gpl-3.0.html",
  "MPL-2.0": "https://opensource.org/licenses/MPL-2.0",
};
function syncLicenseUrl(): void {
  const hit = LICENSE_URLS[sel("license-name").value];
  if (hit) input("license-url").value = hit;
}
type MemField = "groupId" | "artifactId" | "version" | "classifier" | "portalUser" | "filePath" | "licenseUrl" | "scmUrl" | "devId" | "devName" | "devEmail";
interface FormMemory {
  last: Record<MemField, string>;
  histories: Record<MemField, string[]>;
}
function defaultMemory(): FormMemory {
  return {
    last: { groupId: "", artifactId: "", version: "", classifier: "", portalUser: "", filePath: "", licenseUrl: "", scmUrl: "", devId: "", devName: "", devEmail: "" },
    histories: { groupId: [], artifactId: [], version: [], classifier: [], portalUser: [], filePath: [], licenseUrl: [], scmUrl: [], devId: [], devName: [], devEmail: [] },
  };
}
function loadMemory(): FormMemory {
  try {
    const raw = localStorage.getItem(MEM_KEY);
    if (!raw) return defaultMemory();
    const o = JSON.parse(raw) as Partial<FormMemory>;
    const d = defaultMemory();
    if (o.last) {
      (Object.keys(d.last) as MemField[]).forEach((k) => {
        const v = (o.last as Record<string, unknown>)[k];
        if (typeof v === "string") d.last[k] = v;
      });
    }
    if (o.histories) {
      (Object.keys(d.histories) as MemField[]).forEach((k) => {
        const v = (o.histories as Record<string, unknown>)[k];
        if (Array.isArray(v)) {
          d.histories[k] = (v as unknown[]).filter((x): x is string => typeof x === "string" && x.length > 0).slice(0, MEM_MAX);
        }
      });
    }
    return d;
  } catch {
    return defaultMemory();
  }
}
function persistMemory(m: FormMemory): void {
  try {
    localStorage.setItem(MEM_KEY, JSON.stringify(m));
  } catch { /* ignore quota errors */ }
}
function pushHistory(m: FormMemory, field: MemField, value: string): void {
  const v = (value || "").trim();
  if (!v) return;
  m.histories[field] = [v, ...m.histories[field].filter((x) => x !== v)].slice(0, MEM_MAX);
}
function rememberCurrent(): void {
  const v = {
    groupId: input("group-id").value.trim(),
    artifactId: input("artifact-id").value.trim(),
    version: input("version").value.trim(),
    packaging: sel("packaging").value,
    classifier: input("classifier").value.trim(),
    portalUser: input("portal-user").value.trim(),
    filePath: pickedFile,
    licenseUrl: input("license-url").value.trim(),
    scmUrl: input("scm-url").value.trim(),
    devId: input("dev-id").value.trim(),
    devName: input("dev-name").value.trim(),
    devEmail: input("dev-email").value.trim(),
  };
  const m = loadMemory();
  m.last = { groupId: v.groupId, artifactId: v.artifactId, version: v.version, classifier: v.classifier, portalUser: v.portalUser, filePath: v.filePath, licenseUrl: v.licenseUrl, scmUrl: v.scmUrl, devId: v.devId, devName: v.devName, devEmail: v.devEmail };
  try { localStorage.setItem(MEM_KEY + ":packaging", v.packaging); } catch { /* ignore */ }
  try { localStorage.setItem(MEM_KEY + ":licenseName", sel("license-name").value); } catch { /* ignore */ }
  pushHistory(m, "groupId", v.groupId);
  pushHistory(m, "artifactId", v.artifactId);
  pushHistory(m, "version", v.version);
  pushHistory(m, "classifier", v.classifier);
  pushHistory(m, "portalUser", v.portalUser);
  pushHistory(m, "filePath", v.filePath);
  pushHistory(m, "licenseUrl", v.licenseUrl);
  pushHistory(m, "scmUrl", v.scmUrl);
  pushHistory(m, "devId", v.devId);
  pushHistory(m, "devName", v.devName);
  pushHistory(m, "devEmail", v.devEmail);
  persistMemory(m);
  renderMemory();
}
function rememberFile(pth: string): void {
  if (!pth) return;
  const m = loadMemory();
  m.last.filePath = pth;
  pushHistory(m, "filePath", pth);
  persistMemory(m);
  renderMemory();
}
function fillDatalist(id: string, values: string[]): void {
  const dl = document.getElementById(id);
  if (!dl) return;
  dl.innerHTML = "";
  values.forEach((v) => {
    const o = document.createElement("option");
    o.value = v;
    dl.appendChild(o);
  });
}
function renderMemory(): void {
  const m = loadMemory();
  fillDatalist("dl-group-id", m.histories.groupId);
  fillDatalist("dl-artifact-id", m.histories.artifactId);
  fillDatalist("dl-version", m.histories.version);
  fillDatalist("dl-classifier", m.histories.classifier);
  fillDatalist("dl-portal-user", m.histories.portalUser);
  fillDatalist("dl-license-url", m.histories.licenseUrl);
  fillDatalist("dl-scm-url", m.histories.scmUrl);
  fillDatalist("dl-dev-id", m.histories.devId);
  fillDatalist("dl-dev-name", m.histories.devName);
  fillDatalist("dl-dev-email", m.histories.devEmail);
  const rf = document.getElementById("recent-files") as HTMLSelectElement | null;
  if (rf) {
    const cur = rf.value;
    rf.innerHTML = "";
    const ph = document.createElement("option");
    ph.value = "";
    ph.textContent = "\u2014 \u9009\u62e9\u6700\u8fd1\u6587\u4ef6 \u2014";
    rf.appendChild(ph);
    m.histories.filePath.forEach((fp) => {
      const o = document.createElement("option");
      o.value = fp;
      const name = fp.split(/[\\/]/).pop() || fp;
      o.textContent = name + "  |  " + fp;
      o.title = fp;
      rf.appendChild(o);
    });
    if (cur && m.histories.filePath.includes(cur)) rf.value = cur;
    else if (pickedFile && m.histories.filePath.includes(pickedFile)) rf.value = pickedFile;
  }
}
function restoreMemory(): void {
  const m = loadMemory();
  if (m.last.groupId) input("group-id").value = m.last.groupId;
  if (m.last.artifactId) input("artifact-id").value = m.last.artifactId;
  if (m.last.version) input("version").value = m.last.version;
  if (m.last.classifier) input("classifier").value = m.last.classifier;
  if (m.last.portalUser) input("portal-user").value = m.last.portalUser;
  if (m.last.licenseUrl) input("license-url").value = m.last.licenseUrl;
  if (m.last.scmUrl) input("scm-url").value = m.last.scmUrl;
  if (m.last.devId) input("dev-id").value = m.last.devId;
  if (m.last.devName) input("dev-name").value = m.last.devName;
  if (m.last.devEmail) input("dev-email").value = m.last.devEmail;
  try {
    const pkg = localStorage.getItem(MEM_KEY + ":packaging");
    if (pkg) {
      const s = sel("packaging");
      if (Array.from(s.options).some((o) => o.value === pkg)) s.value = pkg;
    }
  } catch { /* ignore */ }
  try {
    const ln = localStorage.getItem(MEM_KEY + ":licenseName");
    if (ln) {
      const s = sel("license-name");
      if (Array.from(s.options).some((o) => o.value === ln)) s.value = ln;
    }
  } catch { /* ignore */ }
  renderMemory();
}
function mergePublishHistory(list: Array<{ group_id?: string; artifact_id?: string; version?: string }>): void {
  if (!list || !list.length) return;
  const m = loadMemory();
  list.slice().reverse().forEach((h) => {
    if (h.group_id) pushHistory(m, "groupId", String(h.group_id));
    if (h.artifact_id) pushHistory(m, "artifactId", String(h.artifact_id));
    if (h.version) pushHistory(m, "version", String(h.version));
  });
  persistMemory(m);
  renderMemory();
}


function log(msg: string) {
  const el = $("log") as HTMLPreElement;
  const t = new Date().toLocaleTimeString();
  el.textContent += `[${t}] ${msg}\n`;
  el.scrollTop = el.scrollHeight;
}

function collect() {
  return {
    file_path: pickedFile,
    group_id: input("group-id").value.trim(),
    artifact_id: input("artifact-id").value.trim(),
    version: input("version").value.trim(),
    packaging: sel("packaging").value,
    classifier: input("classifier").value.trim(),
    portalUsername: input("portal-user").value.trim(),
    portalToken: input("portal-token").value,
    publish_catalog: (input("publish-catalog") as HTMLInputElement).checked,
    license_name: sel("license-name").value,
    license_url: input("license-url").value.trim(),
    scm_url: input("scm-url").value.trim(),
    dev_id: input("dev-id").value.trim(),
    dev_name: input("dev-name").value.trim(),
    dev_email: input("dev-email").value.trim(),
  };
}

async function refreshEnv() {
  try {
    const env = await invoke<any>("cmd_env");
    const badge = $("env-badge");
    if (!env.gpg_available && (env as Record<string, unknown>).gpg_state === "timeout") {
      badge.textContent = "GPG 检测超时（后台响应慢），已跳过，点“重新检测”再试";
      badge.className = "badge warn";
      markGpgCard(false);
      log(`应用数据目录：${env.app_data_dir}`);
      return;
    }
    if (env.gpg_available) {
      badge.textContent = `本机环境正常 · ${env.gpg_version}`;
      badge.className = "badge ok";
      markGpgCard(true);
    } else {
      badge.textContent = "未检测到 gpg：正式发布前请安装 Gpg4win 并生成密钥";
      badge.className = "badge warn";
      markGpgCard(false);
    }
    log(`应用数据目录：${env.app_data_dir}`);
  } catch (e) {
    $("env-badge").textContent = `环境检测失败：${String(e)}`;
  }
}

function markGpgCard(ok: boolean) {
  const card = $("gpg-card");
  if (card) card.classList.toggle("missing", !ok);
}

async function refreshGpgKeys() {
  const box = $("gpg-keys");
  try {
    const keys = await invoke<any[]>("cmd_gpg_keys");
    box.innerHTML = "";
    if (!keys.length) { box.innerHTML = `<div class="item">暂无密钥，请填写姓名邮箱并生成。</div>`; return; }
    keys.forEach((k) => {
      const d = document.createElement("div");
      d.className = "item";
      d.textContent = `${k.user_id} · 指纹 ${k.fingerprint}`;
      box.appendChild(d);
    });
  } catch (e) {
    box.innerHTML = `<div class="item">查询失败：${String(e)}</div>`;
  }
}

async function doGpgInstall() {
  const btn = $("btn-gpg-install") as HTMLButtonElement;
  btn.disabled = true;
  log("开始一键安装 Gpg4win（winget 静默安装，约需 1~3 分钟；若弹出系统权限确认请点“是”）…");
  try {
    const m = await invoke<string>("cmd_gpg_install");
    log(m);
    await refreshEnv();
    await refreshGpgKeys();
  } catch (e) {
    log(`安装失败：${String(e)}`);
  } finally {
    btn.disabled = false;
  }
}

async function doGpgGen() {
  log("正在生成 GPG 密钥…");
  try {
    const fp = await invoke<string>("cmd_gpg_gen_key", {
      name: input("gpg-name").value,
      email: input("gpg-email").value,
      passphrase: input("gpg-pass").value,
    });
    log(`密钥已生成，指纹：${fp}`);
    await refreshEnv();
    await refreshGpgKeys();
  } catch (e) {
    log(`生成失败：${String(e)}`);
  }
}
async function doGpgPublish(): Promise<void> {
  const btn = $("btn-gpg-publish") as HTMLButtonElement;
  btn.disabled = true;
  log("正在发布 GPG 公钥到 keyserver（ubuntu + openpgp），需要联网，约几十秒…");
  try {
    const m = await invoke<string>("cmd_gpg_publish_key");
    log(m);
    await refreshGpgKeys();
  } catch (e) {
    log(`发布公钥失败：${String(e)}`);
  } finally {
    btn.disabled = false;
  }
}
async function loadConfig() {
  try {
    const cfg = await invoke<any>("cmd_load_config");
    const _mem = loadMemory();
    if (!_mem.last.portalUser && cfg.portal_username) {
      input("portal-user").value = cfg.portal_username;
      log(`已载入上次用户名 ${cfg.portal_username}（Token ${cfg.has_token ? "已保存" : "未保存"}）`);
    }
  } catch (e) {
    log(`载入配置失败：${String(e)}`);
  }
}


function warnFilePackagingMismatch(info: any): void {
  try {
    const kind = String(info?.kind ?? "unknown");
    const desc = String(info?.kind_desc ?? kind);
    const el = $("file-info");
    if (desc && el.textContent && !el.textContent.includes(desc)) el.textContent += ` · 类型：${desc}`;
    const pkg = sel("packaging").value;
    if (["jar", "war", "zip"].includes(pkg) && kind !== "zip") {
      log(`提醒：当前 packaging=${pkg} 需要标准 ZIP 格式，但所选文件识别为“${desc}”。直接发布会在 Portal 校验阶段失败，建议先换文件或改 packaging，再点“本地试打包”验证。`);
    } else if (pkg === "tar.gz" && kind !== "gzip") {
      log(`提醒：当前 packaging=tar.gz 需要 GZip 格式，但所选文件识别为“${desc}”。建议先换文件，再点“本地试打包”验证。`);
    }
  } catch { /* ignore */ }
}

async function pickFile(path?: string) {
  try {
    let p = path;
    if (!p) {
      const r = await open({ multiple: false, directory: false });
      if (!r) return;
      p = Array.isArray(r) ? (r[0] as string) : (r as string);
    }
    if (!p) return;
    pickedFile = p;
    rememberFile(p);
    $("file-label").textContent = p;
    const info = await invoke<any>("cmd_file_info", { filePath: p });
    $("file-info").textContent = `文件名 ${info.file_name} · ${info.size} 字节 · SHA256 ${String(info.sha256).slice(0, 16)}…`;
    warnFilePackagingMismatch(info);
    log(`已选择文件：${p}`);
  } catch (e) {
    log(`选择文件失败：${String(e)}`);
  }
}

async function doValidate() {
  const c = collect();
  rememberCurrent();
  try {
    const warnings = await invoke<string[]>("cmd_validate", {
      groupId: c.group_id,
      artifactId: c.artifact_id,
      version: c.version,
      licenseName: c.license_name,
      licenseUrl: c.license_url,
      scmUrl: c.scm_url,
      devId: c.dev_id,
      devName: c.dev_name,
      devEmail: c.dev_email,
    });
    if (!c.file_path) log("提醒：尚未选择待发布文件。");
    log("坐标校验通过。");
    warnings.forEach((w) => log(`提示：${w}`));
  } catch (e) {
    log(`校验未通过：${String(e)}`);
  }
}

async function doDryRun() {
  const c = collect();
  rememberCurrent();
  if (!c.file_path) { log("请先选择文件。"); return; }
  log("开始本地试打包（不上传）…");
  try {
    const r = await invoke<any>("cmd_dry_run", { request: c });
    log(`试打包成功：${r.bundle_path}`);
    (r.files as string[]).forEach((f) => log(` · ${f}`));
    log(`SHA256 ${r.sha256} · ${r.size} 字节`);
  } catch (e) {
    log(`试打包失败：${String(e)}`);
  }
}

async function doPublish() {
  const c = collect();
  rememberCurrent();
  if (!c.file_path) { log("请先选择文件。"); return; }
  const btn = $("btn-publish") as HTMLButtonElement;
  btn.disabled = true;
  log("开始发布到中央仓（主构件→清单），请勿关闭窗口…");
  try {
    const r = await invoke<any>("cmd_publish", { request: c });
    log(`主构件已上传，deployment=${r.deployment_id}，状态=${r.status}`);
    if (r.catalog_version) log(`合集清单已发布：share-catalog:${r.catalog_version}`);
    if (r.catalog_status) log(`${r.catalog_status}`);
    await refreshHistory();
    await refreshCatalog();
  } catch (e) {
    log(`发布失败：${String(e)}`);
  } finally {
    btn.disabled = false;
  }
}

async function doCheckToken() {
  log("正在校验 Portal 凭证（只查状态接口，不上传）…");
  try {
    const m = await invoke<string>("cmd_portal_check", {
      portalUsername: input("portal-user").value.trim(),
      portalToken: input("portal-token").value,
    });
    log(m);
  } catch (e) { log(`凭证校验未通过：${String(e)}`); }
}

async function refreshHistory() {
  try {
    const list = await invoke<any[]>("cmd_history");
    const box = $("history-list");
    box.innerHTML = "";
    if (!list.length) { box.innerHTML = `<div class="item">暂无历史</div>`; return; }
    mergePublishHistory(list);
    list.slice(0, 30).forEach((h) => {
      const d = document.createElement("div");
      d.className = "item";
      d.innerHTML = `<b>[${h.kind}] ${h.group_id}:${h.artifact_id}:${h.version}</b> · ${h.file_name} · ${h.published_at}<br/>deployment=${h.deployment_id} · 状态=${h.status}`;
      box.appendChild(d);
    });
  } catch (e) {
    log(`刷新历史失败：${String(e)}`);
  }
}

async function refreshCatalog() {
  const gid = input("group-id").value.trim();
  if (!gid) { $("catalog-meta").textContent = "请先填写 groupId 再刷新清单"; return; }
  try {
    const c = await invoke<any>("cmd_catalog", { groupId: gid });
    $("catalog-meta").textContent = `清单 ${c.group_id}:${c.artifact_id} · 最新版 ${c.latest_version ?? "无"} · 更新于 ${c.updated_at ?? "-"}`;
    const box = $("catalog-list");
    box.innerHTML = "";
    const entries = (c.entries as any[]) ?? [];
    if (!entries.length) { box.innerHTML = `<div class="item">该 group 暂无清单条目</div>`; return; }
    entries.slice().reverse().slice(0, 30).forEach((e) => {
      const d = document.createElement("div");
      d.className = "item";
      d.innerHTML = `<b>${e.group_id}:${e.artifact_id}:${e.version}</b> · ${e.file_name} · ${e.published_at}<br/>SHA256=${String(e.sha256).slice(0, 20)}… · deployment=${e.deployment_id}`;
      box.appendChild(d);
    });
  } catch (e) {
    log(`刷新清单失败：${String(e)}`);
  }
}

window.addEventListener("DOMContentLoaded", () => {
  $("btn-pick").addEventListener("click", () => pickFile());
  $("btn-validate").addEventListener("click", doValidate);
  $("btn-dryrun").addEventListener("click", doDryRun);
  $("btn-publish").addEventListener("click", doPublish);
  $("btn-history").addEventListener("click", refreshHistory);
  $("btn-gpg-install").addEventListener("click", doGpgInstall);
  $("btn-gpg-page").addEventListener("click", async () => { try { await openUrl("https://gpg4win.org/download.html"); } catch (e) { log(`打开失败：${String(e)}`); } });
  $("btn-gpg-recheck").addEventListener("click", async () => { await refreshEnv(); await refreshGpgKeys(); });
  $("btn-gpg-gen").addEventListener("click", doGpgGen);
  $("btn-gpg-keys").addEventListener("click", refreshGpgKeys);
  $("btn-gpg-publish").addEventListener("click", doGpgPublish);
  $("btn-check-token").addEventListener("click", doCheckToken);
  $("btn-catalog").addEventListener("click", refreshCatalog);
  $("btn-save-token").addEventListener("click", async () => {
    try {
      const m = await invoke<string>("cmd_save_token", {
        portalUsername: input("portal-user").value.trim(),
        portalToken: input("portal-token").value,
      });
      log(m);
    } catch (e) { log(`保存失败：${String(e)}`); }
  });
  const dz = $("dropzone");
  ["dragenter", "dragover"].forEach((ev) => dz.addEventListener(ev, (e) => { e.preventDefault(); dz.classList.add("over"); }));
  ["dragleave", "drop"].forEach((ev) => dz.addEventListener(ev, (e) => { e.preventDefault(); dz.classList.remove("over"); }));
  dz.addEventListener("drop", async (e) => {
    const de = e as DragEvent;
    const f = de.dataTransfer?.files?.[0] as any;
    const p = f?.path as string | undefined;
    if (p) await pickFile(p);
    else log("拖拽未取到本地路径，请改用“选择文件”按钮。");
  });
  restoreMemory();
  try {
    const rf = document.getElementById("recent-files") as HTMLSelectElement | null;
    if (rf) rf.addEventListener("change", (e) => {
      const v = (e.target as HTMLSelectElement).value;
      if (v) void pickFile(v);
    });
    const cb = document.getElementById("btn-clear-memory");
    if (cb) cb.addEventListener("click", () => {
      try { localStorage.removeItem(MEM_KEY); localStorage.removeItem(MEM_KEY + ":packaging"); } catch { /* ignore */ }
      renderMemory();
      log("表单记忆已清空");
    });
    ["group-id", "artifact-id", "version", "classifier", "portal-user"].forEach((id) => {
      input(id).addEventListener("change", rememberCurrent);
    });
    sel("packaging").addEventListener("change", rememberCurrent);
    sel("license-name").addEventListener("change", () => { syncLicenseUrl(); rememberCurrent(); });
    ["license-url", "scm-url", "dev-id", "dev-name", "dev-email"].forEach((id) => {
      input(id).addEventListener("change", rememberCurrent);
    });
  } catch { /* memory UI optional */ }
  void (async () => { await refreshEnv(); await refreshGpgKeys(); })();
  loadConfig();
  refreshHistory();
});
