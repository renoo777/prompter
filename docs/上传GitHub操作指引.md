# 上传到 GitHub 的操作指引

> 状态（2026-10-08）：**本地已全部就绪，只差 GitHub 认证。**
> 仓库已初始化、已提交（`70d24ca`）、工作区干净。
> 上传内容已逐项核对：**170 个文件 / 2.43 MB**，该带的全带、不该带的全排。

---

## 一、为什么我不能替你推

这台机器上：

| 项 | 状态 |
|---|---|
| `git` 可执行文件 | ✅ 有（`~/.workbuddy/binaries/PortableGit/versions/1.2.0/cmd/git.exe`） |
| git 用户名 / 邮箱 | ❌ **未配置** |
| 凭据助手（credential.helper） | ❌ **未配置** |
| `gh` CLI | ❌ **没装** |
| GitHub 认证 | ❌ **从来没有过** |
| 网络 | ✅ `api.github.com` 返回 200 |

**⇒ 推送需要你的 GitHub 账号授权，这一步我不能代做。** 下面给你两条路，选一条。

---

## 二、路线 A：GitHub 网页建仓 + 命令行推送（推荐）

### 第 1 步：在 GitHub 网页建一个**空**仓库

1. 打开 <https://github.com/new>
2. Repository name 填 `prompter`（或你想要的名字）
3. ⚠️ **不要勾选** "Add a README file" / "Add .gitignore" / "Choose a license"
   —— 本地已经有内容了，勾了会冲突
4. 点 **Create repository**
5. 复制仓库地址，形如 `https://github.com/<你的用户名>/prompter.git`

### 第 2 步：在本机告诉 git 你是谁

```bash
git config --global user.name "你的GitHub用户名"
git config --global user.email "你的GitHub邮箱"
```

### 第 3 步：加远端并推送

```bash
cd /d/agent/Claw/2026-09-25-14-13-48/prompter
git remote add origin https://github.com/<你的用户名>/prompter.git
git push -u origin master
```

> 推送时会要求输入 **Personal Access Token**（不是账号密码，GitHub 2021 年起已禁用密码推送）。
> 取 token：GitHub → 右上头像 → Settings → Developer settings →
> Personal access tokens → Tokens (classic) → Generate new token
> 勾选 `repo` 权限即可。

> ⚠️ 若提示 `master` 不存在，把 `master` 换成 `main`。

---

## 三、路线 B：你不想配 token，用 SSH

```bash
# 1. 看有没有 SSH 密钥
ls ~/.ssh/id_rsa.pub 2>/dev/null || echo "没有，需要先 ssh-keygen -t ed25519"

# 2. 把公钥贴到 GitHub → Settings → SSH and GPG keys → New SSH key
cat ~/.ssh/id_rsa.pub

# 3. 改用 SSH 地址
git remote add origin git@github.com:<你的用户名>/prompter.git
git push -u origin master
```

---

## 四、Mac 上怎么下载（你要的那个动作）

推送成功后，在 Mac 的终端里：

```bash
git clone https://github.com/<你的用户名>/prompter.git
cd prompter

# 装 Xcode 命令行工具（链接 ObjC 框架必需，只在第一次）
xcode-select --install

# 然后照 docs/macOS上机测试清单.md 走
```

**或者**在 GitHub 仓库页面点绿色 **Code → Download ZIP**。但 **ZIP 方式不推荐**，原因见下。

> ⚠️ **优先用 `git clone`，别用 Download ZIP。**
> ZIP 会丢掉 git 历史，而且 `build.sh` 没有可执行位（ZIP 解压后是 `-rw-r--r--`），
> 直接 `./build.sh` 会报 **Permission denied**，还得手动 `chmod +x build.sh *.sh`。
> `git clone` 会保留文件模式，一条命令就能跑。

---

## 五、Mac 上第一次跑的完整命令

```bash
git clone https://github.com/<你的用户名>/prompter.git
cd prompter

xcode-select --install        # 第一次才需要
npm install                   # 前端依赖
./build.sh                    # 编译检查 + 跑单测 + 打 .app
open src-tauri/target/release/bundle/macos/Prompter.app
```

首次打开会提示「来自身份不明的开发者」——**这是必然的**（用了未文档化的私有 API，
无法签名与公证）。在「系统设置 → 隐私与安全性」点「仍要打开」。

> **测试前请先读 [`docs/macOS上机测试清单.md`](docs/macOS上机测试清单.md)。**
> 里面第 0 步就要你报 `sw_vers` 的版本号，第 3 步是最高风险点（闪退），
> 第 5 步有张逐个会议软件的记录表——**那 6 行数据是这个项目现在最缺的东西**。

---

## 六、这次推了什么（已核对）

| 检查项 | 结果 |
|---|---|
| 文件数 | 170 |
| 体积 | **2.43 MB** |
| `src-tauri/target/`（**18 GB**） | ✅ 已排除 |
| `node_modules/` | ✅ 已排除 |
| `dist/` | ✅ 已排除 |
| 临时截图（`_bbox_test.png` / `stealth_ref.png` / `tmp-probe.png`） | ✅ 已排除 |
| `README.md`、`build.sh`、`run-dev.sh`、`Cargo.lock`、`tauri.conf.json`、`icon.icns` | ✅ 都在 |
| 8 份文档（含 4 份本轮新写的） | ✅ 都在 |
| `docs/shots/` 8 张文档配图 | ✅ 都在 |
| `build.sh` 入库换行符 | ✅ **LF**（`.gitattributes` 生效，防 Mac 上 `bad interpreter`） |
| 敏感文件 | ✅ 无（`config.json` 是运行时生成的，不在仓库里） |
| `macOS 后端离线编译校验` | ✅ exit=0 |

---

## 七、如果推送时出问题

| 现象 | 处置 |
|---|---|
| `fatal: Authentication failed` | token 过期或没给 `repo` 权限 |
| `remote: Permission denied` | 仓库名拼错，或 token 属于另一个账号 |
| `src refspec master does not match any` | 分支名是 `main`，改用它 |
| `Updates were rejected` | 远端有内容（建仓时勾了 README）⇒ 用路线 A 的"不要勾选"重来 |
| 传到一半失败 | 体积只有 2.43MB，正常不会；重跑同一条 `push` 即可续 |
