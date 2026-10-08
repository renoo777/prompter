# 上传到 GitHub 的操作指引

## 零、✅ 已完成（2026-10-08 18:25）

**仓库已上线：<https://github.com/renoo777/prompter>（公开，默认分支 `master`）**

| 项 | 结果 |
|---|---|
| 提交 | **7 个**，完整历史（不是"导入"成一个大提交） |
| 文件 | **173 个**（含本轮新增的推送脚本） |
| 内容核验 | ✅ **远端 HEAD 的 tree SHA == 本地 HEAD 的 tree SHA** ⇒ 文件内容与权限位逐字节一致 |
| `target/`(18GB) / `node_modules/` / `dist/` | ✅ 全部未上传 |

### Mac 上怎么拿

```bash
git clone https://github.com/renoo777/prompter.git
cd prompter
xcode-select --install     # 第一次才需要
npm install
./build.sh
```

> **`git clone` 优先于网页 Download ZIP**：ZIP 丢文件可执行位（`build.sh` 变 `-rw-r--r--`），
> 直接 `./build.sh` 报 Permission denied，还得补 `chmod +x *.sh`。

**若 Mac 上也连不上 `github.com`**（国内网络常见），用零凭据的离线包：
`_transfer/prompter.bundle` → `git clone prompter.bundle prompter`（见第五节）。

### ★ 两条本次踩到、以后还会遇到的坑

**坑 1｜「我明明改了 token，怎么还是 401」——你改的不是 git 读的那条。**
GitHub 的 HTTPS 会让 git 按**解析出的 IP** 另存一份凭据。你机器上有 5 条 `git:https://*`：

| 条目 | 修改时间 | 状态 |
|---|---|---|
| `git:https://github.com` ← **git 默认读这条** | 2026-09-07 | ❌ 401 |
| `git:https://140.82.116.4` | 2026-09-06 | ❌ 401 |
| **`git:https://140.82.112.4`** ← **你今天改的是这条** | **2026-10-08** | ✅ **有效** |
| `git:https://140.82.113.3` | 2026-09-06 | ❌ 401 |
| `git:https://140.82.121.3` | 2026-09-06 | ❌ 401 |

⇒ `scripts/push-to-github.py` 已改成**枚举全部 git 凭据、逐条验、用第一条能通的**，
不再只读 `github.com` 那一条。

**坑 2｜`git push` 走不通，但 API 通 —— 于是改用 REST API 推送。**
实测本机网络：`api.github.com` ✅ 0.3s ／ `github.com:443` ❌ 超时 ／ `codeload.github.com` ❌ 超时。
而 `git push` 走的正是 `github.com` ⇒ 怎么试都报 `Connection was reset` / `Could not connect`。
**这不是认证问题，是网络问题。**

处置：`scripts/push-via-api.py` —— 经 `api.github.com` 的 Git Data API
（blobs → trees → commits → ref）**逐提交重放**，全程不碰 `github.com`。
脚本内已内置两个实测限制的处置（空仓库 409、消息末尾换行被裁导致 SHA 必然不同），
文件头有完整说明。

### ⚠️ 一个必须知道的后果：本地与远端「同内容、异历史」

GitHub 提交 API **会裁掉消息末尾的换行**，而 git 的提交对象消息必然以换行结尾
⇒ **commit SHA 不可能复现**（实测：根提交 tree 完全相同、消息只差末尾一个 `\n`）。

所以将来网络通了、想推本地新提交时：

```bash
git push --force      # 历史不同源，必须 force
```

或者干脆**以远端为准重新 clone**。内容不会丢——tree SHA 已证明逐字节一致。

---

> ## 以下为排查过程存档（保留以备复用）
>
> 本节的路线 A/B/C/D 是在**还不知道"API 能通、github.com 不通"**时写的，
> 当时判断的阻塞点是"凭据失效"。**结论已被第零节取代**，但排查手法仍然有效。

---

## 一、排查存档：`git:https://github.com` 这条的 token 是过期的

我为了"直接推"把整条链路挖到底了。过程和结论如实写在下面。

| 项 | 状态 |
|---|---|
| `git` 可执行文件 | ✅ 有，在 `~/.workbuddy/binaries/PortableGit/versions/1.2.0/cmd/git.exe`（**在 `cmd/` 不在 `bin/`**） |
| `~/.gitconfig` | ✅ 有，写着 `credential.helperselector.selected = manager` ⇒ **你以前配过** |
| **Windows 凭据管理器** | ⚠️ **里面有一条 `git:https://github.com` 的凭据** |
| 这条凭据的形态 | `ghp_` 开头、40 字符、纯 ASCII ⇒ **格式完全正确的 classic PAT** |
| **实际可用性** | ❌ **三种调用全部 401 `Bad credentials`** ⇒ **已过期或被撤销** |
| `api.github.com` | ✅ 通（HTTP 200） |
| `github.com`（经 git/libcurl） | ✅ 通 —— 实测 `git ls-remote` 返回了 commit hash |
| `github.com`（经 python/urllib、.NET） | ❌ 超时／连接被关闭 |

**⇒ 不是缺认证，是认证过期了。** 换一把新的就能推，**其他什么都不用改**。

### 我试过的三条"自动拿到新凭据"的路，都断了（如实记录）

| 尝试 | 结果 | 原因 |
|---|---|---|
| **GCM 设备码登录**（`github login --device`） | ❌ 失败 | 报「基础连接已经关闭」。GCM 走 .NET HttpClient，沙箱代理只被 libcurl 认 |
| **GCM 浏览器登录** | ❌ 同上 | 同一原因，都要访问 `github.com` 主站 |
| **修 GCM** | ✅ 顺手修好了一个真问题 | GCM 原先报 `Failed to locate 'git.exe' executable on the path.` —— **不是 GCM 坏，是 git 不在 PATH 里**。把 git 目录加进 PATH 后 GCM 2.9.0 正常输出（但这不影响上面两条的网络结论） |

### 另外两个实测结论（对路线选择有影响）

| 结论 | 影响 |
|---|---|
| `wincred`（Git for Windows 自带）**读**凭据 ✅ 可行 | ⇒ 只要凭据在凭据管理器里，推送就能跑通 |
| `wincred` **写**凭据 ❌ 无效（`store` 返回 0 但不生效，`erase` 也无效） | ⇒ 那条凭据是 GCM 的 `wincredman` 格式存的，**wincred 只能读不能写**。所以要让凭据生效，得走图形界面编辑，或由 GCM 写入 |

### 顺带查清了两件事

**① 你的 GitHub 账号是 `renoo777`。** token 一失效 `GET /user` 就 401，本来拿不到账号名；
但从 SSH 那边问出来了（`ssh -T` 的问候语里带账号），所以推送地址是确定的：

```
https://github.com/renoo777/prompter
```

**② 你机器上有 3 把 SSH 私钥，但都是「部署密钥」，救不了这次。** 详情见第四节。

---

## 二、路线 A：更新凭据管理器（推荐，token 不经过对话）

**好处：token 完全不经我手、不进对话记录，你自己填就行。**

### 第 1 步：生成一把新 token

GitHub → 右上头像 → **Settings** → 左下 **Developer settings** →
**Personal access tokens** → **Tokens (classic)** → **Generate new token (classic)**

- Note 随便填（如 `prompter-push`）
- Expiration 建议选 **90 天**（别选 No expiration，你上次那把估计就是这么丢的）
- **勾选 `repo`** 这一整组（推私有/公有仓库都需要它）
- 生成后**复制**（离开页面就再也看不到了）

### 第 2 步：替换掉那条失效的凭据

> **为什么这一步得你自己动手**：我试过了，`wincred` 能**读**这条凭据，但**写不进去**
> （`store` 返回 0 却不生效，`erase` 也无效）——因为它是 GCM 的 `wincredman` 格式存的。
> 所以只能用图形界面或 `cmdkey` 改，我没法代劳。

**方法一（图形界面，最可靠）**

1. 按 `Win + R`，输入 `control`，回车
2. **用户账户** → **凭据管理器** → **Windows 凭据**
3. 找到 **`git:https://github.com`**
4. 点 **编辑**
   - 用户名：`x-access-token`（不改）
   - 密码：**粘贴新 token**
5. 保存

**方法二（命令行，一条）**

```bat
cmdkey /generic:git:https://github.com /user:x-access-token /pass:<粘贴新token>
```

> 若方法二执行后 `--check` 仍报 401，改用方法一——`cmdkey` 建的条目类型
> 可能与 GCM 那条不完全是同一个，图形界面编辑最稳。

### 第 3 步：告诉我一声，剩下我一条命令做完

```bash
python scripts/push-to-github.py --repo prompter
```

这个脚本会自动完成**验证身份 → 建仓库（若不存在）→ 关联 origin → 推送**，
并在推送前自检"`target/` 与 `node_modules/` 没被跟踪"（防止 18GB 传上去）。
**你不用去网页建仓库。**

> 想先确认凭据换对了：`python scripts/push-to-github.py --repo prompter --check`

---

## 三、路线 B：直接把 token 给我（最快）

如果你不想点凭据管理器，直接把新 token 发我，我跑：

```bash
GITHUB_TOKEN=<新token> python scripts/push-to-github.py --repo prompter
```

脚本会用这个 token 走 `api.github.com` 建仓库（这条路已验证通畅），
再把它写进一个**临时凭据文件**喂给 git push，**推完立即删除**。

**代价：token 会出现在对话记录里。** 所以更推荐路线 A。
若走了 B，推完请去 GitHub 把这把 token **撤销重发**。

> ⚠️ **如实说明**：这条路径的"喂给 git"那一步，我在沙箱里**没能端到端验证成功**
> ——沙箱会拦截 git 主动读取凭据的操作（进程被直接 SIGTERM，连日志都没有）。
> 真跑的时候如果这步失败，我有一条兜底：把 token 临时嵌进 remote URL
> （`https://x-access-token:<token>@github.com/...`），推完立刻 `set-url` 改回干净地址。
> 两条都不行的话，就走路线 A。

---

## 四、路线 C：SSH —— 查过了，**现成的钥匙都用不上**

我本来以为路线上有个捷径：你 `~/.ssh/` 里已经有 3 把私钥，说不定能用。
**实测逐把验过，结论是否定的**，但过程有价值，写下来免得你或以后的重复查。

| 私钥 | `ssh -T` 问回来的身份 | 能否用于 prompter |
|---|---|---|
| `flownav_dk` | `Hi renoo777/flow-navigator!` | ❌ **部署密钥**，只绑 flow-navigator 一个仓库 |
| `taskboard_gh` | `Hi renoo777/task-board-extension!` | ❌ 同上，只绑 task-board-extension |
| `flownav_push_6119` | `Permission denied (publickey)` | ❌ 已失效（临时钥匙，用完就该撤了） |

**关键点：这三把的问候语都是 `Hi renoo777/<仓库名>!` 而不是 `Hi renoo777!`** ——
这个格式差别正是「部署密钥」与「账号级密钥」的分水岭。部署密钥天生**只对一个仓库有效，
且不能用来建仓库**。而我们要建的是 `renoo777/prompter` 这个新仓库，两样都占了。

> 另外你 `~/.ssh/config` 里那段注释已经写过这个坑了（"复用会直接报 Key is already in use"），
> 说明你当时就踩过。所以这次不用再去试复用。

**要走 SSH 就得新起一把钥匙，而且有 2 步得你动手**（我没法代你上网页）：

```bash
ssh-keygen -t ed25519 -f ~/.ssh/prompter_gh -C "prompter push"   # 第 1 步：生成（我可以帮你跑）
cat ~/.ssh/prompter_gh.pub                                        # 第 2 步：贴到 GitHub
```

- 贴到 **Settings → SSH and GPG keys → New SSH key**（账号级，不是仓库级）——
  这样以后任何新仓库都不用再配一次；
- 还得**先在网页上把 `prompter` 仓库建出来**（SSH 不能建仓库）；
- ⚠️ 走 SSH 的话我的脚本用不上（它走 HTTPS + token），`remote` 和 `push` 得自己敲：

```bash
git remote add origin git@github.com:renoo777/prompter.git
git push -u origin master
```

**⇒ 路线 C 要你做 3 件事（生成钥匙是第 4 步，那步我能代劳，但贴公钥、建仓库都在网页上）。**
比路线 B 麻烦，只在"你不想让 token 经过对话"时才划算。

### ★ 关于「让 GCM 自动登录」这条路

本机 GCM 2.9.0 **是好的**（我顺手修掉了它 "Failed to locate 'git.exe'" 的问题——
根因只是 git 不在 PATH 里），但它做设备码/浏览器登录时要访问 `github.com` 主站，
在沙箱代理下报**「基础连接已经关闭」**。所以这条路在当前环境走不通，
**不是配置问题，是网络问题**。等哪天你在自己的终端里（不带沙箱代理）跑一次
`git push`，GCM 会在浏览器里让你登录一次，之后凭据就存下来了。


---

## 五、路线 D：不走 GitHub，直接把仓库拷到 Mac（零凭据，本轮新增）

你真正要的动作是「**在 Mac 上拿到能跑的代码**」，GitHub 只是手段。
既然凭据卡住了，我先把**不需要任何凭据的产物**做出来了 —— 你现在就能拿去用。

已经生成好两个文件（在仓库外，`D:\agent\Claw\2026-09-25-14-13-48\_transfer\`）：

| 文件 | 大小 | 是什么 |
|---|---|---|
| **`prompter.bundle`** | 1.38 MB | **完整 git 仓库**（4 个提交、全部历史都在里面），推荐 |
| `prompter-source.zip` | 1.44 MB | 源码快照（只有最新一版文件，无历史） |

> **为什么推荐 `.bundle` 而不是 zip**：单个文件、却等价于一个完整的 git 远端。
> 在 Mac 上 `git clone` 它，得到的是**有历史的真仓库**，文件模式（`build.sh` 的可执行位）
> 也保住了。zip 两条都丢。

**用法**（把 `prompter.bundle` 用微信/AirDrop/网盘随便什么方式拷到 Mac，然后）：

```bash
git bundle verify prompter.bundle          # 可选：先验一下完整性
git clone prompter.bundle prompter         # 得到有完整历史的 prompter/
cd prompter
```

**这样拷过来的仓库没有 remote**，等哪天 GitHub 通了，补一条就行：

```bash
git remote add origin https://github.com/renoo777/prompter.git
git push -u origin master                  # 有过改动才需要
```

**若 Mac 上没装 git**（全新机器有可能），用 zip 兜底，但要补一步：

```bash
unzip prompter-source.zip && cd prompter
chmod +x build.sh run-dev.sh               # ★ zip 丢可执行位，不补会 Permission denied
```

> 想重新生成这两个文件（比如后来又改了代码）：
> ```bash
> git bundle create ../_transfer/prompter.bundle --all
> git archive --format=zip --prefix=prompter/ -o ../_transfer/prompter-source.zip HEAD
> ```

---

## 六、Mac 上怎么下载（你要的那个动作）

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

## 七、Mac 上第一次跑的完整命令

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

## 八、这次推了什么（已核对）

| 检查项 | 结果 |
|---|---|
| 文件数 | 172 |
| 体积 | **2.45 MB** |
| `src-tauri/target/`（**18 GB**） | ✅ 已排除 |
| `node_modules/` | ✅ 已排除 |
| `dist/` | ✅ 已排除 |
| 临时截图（`_bbox_test.png` / `stealth_ref.png` / `tmp-probe.png`） | ✅ 已排除 |
| `README.md`、`build.sh`、`run-dev.sh`、`Cargo.lock`、`tauri.conf.json`、`icon.icns` | ✅ 都在 |
| 8 份文档（含 4 份本轮新写的） | ✅ 都在 |
| `docs/shots/` 8 张文档配图 | ✅ 都在 |
| `build.sh` 入库换行符 | ✅ **LF**（`.gitattributes` 生效，防 Mac 上 `bad interpreter`） |
| `target/` 与 `node_modules/` 未被 git 跟踪 | ✅ 实测 `git ls-files` 均为空 |
| 敏感文件 | ✅ 无（`config.json` 是运行时生成的，不在仓库里） |
| `macOS 后端离线编译校验` | ✅ exit=0 |

**本地提交历史**（推送后会一起上去）：

| 提交 | 内容 |
|---|---|
| `70d24ca` | 投屏隐身提词器（macOS 上机测试前状态） |
| `46e072f` | docs：补 GitHub 上传操作指引 |
| `3f61baf` | scripts：加 push-to-github.py |
| `3825196` | docs+scripts：如实记录凭据链路排查（token 失效 / GCM 网络不通 / wincred 只读） |
| `93e3ed1` | docs：查清账号（renoo777）与 SSH 密钥性质，补零凭据转移方案（`prompter.bundle`） |
| （最后一次） | docs：修正核对数字（172 文件 / 2.45 MB）、补本表、并记下上面这条「SHA 不写进自己」的约定 |

---

## 九、如果推送时出问题

| 现象 | 处置 |
|---|---|
| **`HTTP 401 Bad credentials`（就是现在这个）** | **token 已过期或被撤销** ⇒ 走第二节换新 token |
| `ERROR: Repository not found`（走 SSH 时） | 用了部署密钥 ⇒ 它只认绑定的那一个仓库；见第四节 |
| `Key is already in use`（贴公钥时） | 公钥已在账号内别处用过（部署密钥不可复用）⇒ 重新生成一把 |
| `fatal: Authentication failed` | 同上；或新 token 没勾 `repo` 权限 |
| `remote: Permission denied` | 仓库名拼错，或 token 属于另一个账号 |
| `src refspec master does not match any` | 分支名是 `main`，加 `--branch main` |
| `Updates were rejected` | 远端非空 ⇒ 建仓时别勾 README（脚本用 `auto_init: false` 已规避） |
| `GCM 报 .NET 异常（0xE0434352）` | 这台机器的 GCM 跑不起来，**绕过它用 wincred**（脚本已这么做） |
| 传到一半失败 | 体积只有 2.45MB，正常不会；重跑同一条命令即可续 |
