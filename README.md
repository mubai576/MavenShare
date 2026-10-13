# MavenShare

把任意指定文件发布到 Maven 中央仓的 Tauri 小工具。选文件、填 GAV，一键生成合规 bundle 并直调 Central Portal API 上传；成功后自动更新并发布合集清单（share-catalog）。

## 快速开始

1. `npm install`
2. `npm run tauri dev` 本地运行
3. 先看应用内“新手引导”：Sonatype 注册验证域名、Portal 生成 User Token、安装 Gpg4win 并生成密钥。
4. 选文件 → 填 groupId/artifactId/version → 填 License/SCM/开发者 → 校验 → 本地试打包 → 发布。

## 表单记忆

- 组坐标、License/SCM/开发者、Portal 用户名、最近文件都存 localStorage，下次打开自动回填，支持历史下拉选择；Token 不进记忆，走后端安全存储。
- “清空记忆”按钮可一键清除。

## 发布合规校验（本地先拦）

- jar/war/zip 类 packaging 要求源文件是标准 ZIP（`.exe` 冒充 jar 会被拦截并提示）；tar.gz 要求 GZip 魔数。
- bundle 自动含：主产物 + pom（含 licenses/scm/developers）+ sources/javadoc + 全量 GPG 签名 + 每个文件的 `.md5` / `.sha1`。
- GPG 公钥需先用“发布公钥到 Keyserver”发布，全网同步要几分钟到几小时，否则 Portal 报 Invalid signature。

## 清单规则

- 清单固定为同 groupId 下的 `share-catalog`，JSON，含 GAV、文件名、发布时间、大小、SHA256。
- 中央仓 release 不可覆盖同版本，清单用递增版本号实现“覆盖最新版”体验，界面默认展示最新一份。发布失败（FAILED）的 deployment 不占坐标，在 Portal 点 Drop 清掉即可重发同版本。

## 验证

- `cargo test` 后端单测
- `npm run build` 前端构建
- `cargo check` Rust 编译检查
