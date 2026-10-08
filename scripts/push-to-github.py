#!/usr/bin/env python3
"""把当前仓库推送到 GitHub —— 认证 → 建仓 → 关联 → 推送，一条命令走完。

为什么需要这个脚本（而不是手敲三条 git 命令）：
  1. 这一步最容易卡在「远端仓库还没建」——脚本会用 API 自动建，省掉去网页点一遍。
  2. 建仓 + 关联 + 推送之间有依赖（必须先有仓库才能 add remote），
     手敲容易在中途出错后留下半截状态（remote 加了但仓库没建，再跑就报
     "repository not found"，看着像权限问题，其实是顺序问题）。
  3. 顺带做推送前的自检：把不该上传的东西（target/、node_modules/）挡在推送前，
     而不是等 GitHub 拒收或传了 18GB 才发现。

token 的来源（按优先级，**逐条验证、用第一条能通的**）：
  1. 环境变量 GITHUB_TOKEN / GH_TOKEN
  2. Windows 凭据管理器里**全部** `git:https://*` 条目
     ★ 不只是 `git:https://github.com` —— GitHub 的 HTTPS 会让 git 按解析出的 IP
       另存一份凭据（本机实测有 4 条 140.82.x.x）。用户改了一条，git 未必读它。
  **token 只在内存里用、不打印；喂给 git 时写进临时凭据文件，推完立即删除。**

用法：
    python scripts/push-to-github.py --repo prompter            # 自动找可用凭据
    python scripts/push-to-github.py --repo prompter --check    # 只验凭据，不推送
    python scripts/push-to-github.py --repo prompter --private  # 建私有仓库
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)

# PortableGit 的 git.exe 在 cmd/ 下（不是 bin/），且常常不在 PATH 里。
GIT_CANDIDATES = [
    r"C:\Users\26611\.workbuddy\binaries\PortableGit\versions\1.2.0\cmd\git.exe",
    "git",
]


def find_first(paths):
    for p in paths:
        if os.path.sep in p:
            if os.path.exists(p):
                return p
        else:
            return p  # 交给 PATH 解析
    return None


GIT = find_first(GIT_CANDIDATES)


def clean_env():
    """safe-delete 垫片（PYTHONPATH）与 NODE_OPTIONS 会让子进程行为异常。"""
    e = dict(os.environ)
    for k in ("PYTHONPATH", "NODE_OPTIONS"):
        e.pop(k, None)
    e["GIT_TERMINAL_PROMPT"] = "0"   # 别在无人值守时挂在输入上
    e["GCM_INTERACTIVE"] = "never"   # 别弹 GUI
    return e


def git(*args, check=True):
    p = subprocess.run([GIT, *args], cwd=ROOT, capture_output=True, text=True,
                       encoding="utf-8", errors="replace", env=clean_env())
    if check and p.returncode != 0:
        raise RuntimeError("git %s 失败(rc=%s):\n%s\n%s"
                           % (" ".join(args), p.returncode, p.stdout, p.stderr))
    return p


def cred_manager_tokens():
    """枚举 Windows 凭据管理器里**所有** git 凭据 → [(target, token), ...]

    ★ 为什么不能只读 `git:https://github.com`（2026-10-08 真踩）：
    GitHub 的 HTTPS 会让 git 按**解析出的 IP** 另存一份凭据 —— 实测本机有
    `git:https://140.82.112.4` / `140.82.116.4` / `140.82.113.3` / `140.82.121.3`
    共 4 条。用户去凭据管理器里"改那条今天动过的"，很可能改的是 IP 那条，
    而 git 默认读的仍是 `github.com` 那条（旧的）⇒ 症状是
    **"我明明改了 token，怎么还是 401"**。
    ⇒ 正确做法：把全部列出来、逐条验，用第一条能通的。
    """
    if os.name != "nt":
        return []
    try:
        import ctypes
        import ctypes.wintypes as wt
    except Exception:
        return []

    class FILETIME(ctypes.Structure):
        _fields_ = [("dwLowDateTime", wt.DWORD), ("dwHighDateTime", wt.DWORD)]

    class CREDENTIAL_ATTRIBUTEW(ctypes.Structure):
        _fields_ = [("Keyword", wt.LPWSTR), ("Flags", wt.DWORD),
                    ("ValueSize", wt.DWORD), ("Value", ctypes.POINTER(ctypes.c_byte))]

    class CREDENTIALW(ctypes.Structure):
        _fields_ = [
            ("Flags", wt.DWORD), ("Type", wt.DWORD),
            ("TargetName", wt.LPWSTR), ("Comment", wt.LPWSTR),
            ("LastWritten", FILETIME),
            ("CredentialBlobSize", wt.DWORD),
            ("CredentialBlob", ctypes.POINTER(ctypes.c_byte)),
            ("Persist", wt.DWORD), ("AttributeCount", wt.DWORD),
            ("Attributes", ctypes.POINTER(CREDENTIAL_ATTRIBUTEW)),
            ("TargetAlias", wt.LPWSTR), ("UserName", wt.LPWSTR),
        ]

    advapi32 = ctypes.WinDLL("advapi32", use_last_error=True)
    advapi32.CredEnumerateW.argtypes = [
        wt.LPCWSTR, wt.DWORD, ctypes.POINTER(wt.DWORD),
        ctypes.POINTER(ctypes.POINTER(ctypes.POINTER(CREDENTIALW)))]
    advapi32.CredEnumerateW.restype = wt.BOOL
    advapi32.CredFree.argtypes = [ctypes.c_void_p]

    count = wt.DWORD(0)
    pcreds = ctypes.POINTER(ctypes.POINTER(CREDENTIALW))()
    if not advapi32.CredEnumerateW(None, 0, ctypes.byref(count), ctypes.byref(pcreds)):
        return []
    out = []
    try:
        for i in range(count.value):
            c = pcreds[i].contents
            target = c.TargetName or ""
            if not target.startswith("git:https://"):
                continue
            if not (c.CredentialBlobSize and c.CredentialBlob):
                continue
            # ★ 必须 string_at：CredentialBlob 是有符号 c_byte 指针，
            #   转 bytearray 会在 >127 的字节上抛 ValueError。
            blob = ctypes.string_at(c.CredentialBlob, c.CredentialBlobSize)
            tok = blob.decode("utf-16-le", "replace").rstrip("\x00").strip()
            if tok:
                out.append((target, tok))
    finally:
        advapi32.CredFree(pcreds)
    return out


def gather_candidates():
    """按优先级收集候选凭据 → [(来源说明, token, 是否来自环境变量), ...]"""
    cands = []
    for k in ("GITHUB_TOKEN", "GH_TOKEN"):
        v = (os.environ.get(k) or "").strip()
        if v:
            cands.append((k + " 环境变量", v, True))
    # 环境变量最优先；其余按凭据管理器里 git 凭据的顺序（github.com 一般最前）
    for target, tok in cred_manager_tokens():
        cands.append(("凭据管理器 → " + target, tok, False))
    return cands


def api(path, token, method="GET", payload=None):
    url = "https://api.github.com" + path
    data = json.dumps(payload).encode("utf-8") if payload is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Authorization", "Bearer " + token)
    req.add_header("Accept", "application/vnd.github+json")
    req.add_header("X-GitHub-Api-Version", "2022-11-28")
    req.add_header("User-Agent", "prompter-push")
    if data:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return r.status, json.loads(r.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        raw = e.read().decode("utf-8", "replace")
        try:
            return e.code, json.loads(raw)
        except Exception:
            return e.code, {"raw": raw[:300]}
    except Exception as e:
        return None, {"error": "%s: %s" % (type(e).__name__, e)}


def preflight():
    """推送前自检：别把 18GB 的构建产物送上去。"""
    bad = []
    for pat in ("src-tauri/target", "node_modules"):
        p = git("ls-files", pat, check=False)
        if (p.stdout or "").strip():
            bad.append(pat)
    total = git("ls-files", check=False).stdout.count("\n")
    print("  已跟踪文件数：%d" % total)
    if bad:
        print("  ✗ 有不该跟踪的目录：%s" % ", ".join(bad))
        print("    → 先修 .gitignore 再推，否则会把构建产物传上去")
        return False
    print("  ✓ target/ 与 node_modules/ 未被跟踪")
    return True


def main():
    ap = argparse.ArgumentParser(description="推送当前仓库到 GitHub")
    ap.add_argument("--repo", default="prompter", help="仓库名（默认 prompter）")
    ap.add_argument("--branch", default=None, help="分支名（默认读当前分支）")
    ap.add_argument("--private", action="store_true", help="建私有仓库")
    ap.add_argument("--check", action="store_true", help="只验凭据，不推送")
    ap.add_argument("--skip-preflight", action="store_true", help="跳过推送前自检")
    args = ap.parse_args()

    print("=" * 60)
    print("推送到 GitHub")
    print("=" * 60)

    if not GIT:
        print("✗ 找不到 git")
        return 2

    cands = gather_candidates()
    if not cands:
        print("✗ 没有找到任何候选凭据。")
        print("  → 设 GITHUB_TOKEN 环境变量，或往 Windows 凭据管理器里加一条")
        print("    git:https://github.com （用户名填 x-access-token，密码填 PAT）")
        return 3

    print("找到 %d 条候选凭据，逐条验证…" % len(cands))
    token = src = None
    from_env = False
    me = {}
    seen = set()
    tried = []
    for label, tok, is_env in cands:
        if tok in seen:      # 同一把 token 在多个条目里出现，只验一次
            continue
        seen.add(tok)
        st, me = api("/user", tok)
        if st == 200:
            token, src, from_env = tok, label, is_env
            break
        tried.append((label, st, me.get("message", "") if isinstance(me, dict) else ""))
    for label, st, msg in tried:
        print("  ✗ %s → HTTP %s %s" % (label, st, msg))
    if not token:
        print("\n✗ 全部候选凭据都无效 ⇒ 需要一把新的 token")
        print("  → Settings → Developer settings → Personal access tokens → 勾 repo 权限")
        return 4
    login = me["login"]
    print("✓ 已认证：%s" % login)
    print("  token 来源：%s" % src)

    st, repo = api("/repos/%s/%s" % (login, args.repo), token)
    if st == 200:
        print("✓ 仓库已存在：%s" % repo["html_url"])
        if repo.get("size", 0) > 0:
            print("  ⚠️ 远端非空（size=%s KB）—— push 可能被拒，需先处理历史"
                  % repo.get("size"))
    elif st == 404:
        if args.check:
            print("· 仓库不存在（--check 模式，不创建）")
            return 0
        print("· 仓库不存在，正在创建…")
        st2, created = api("/user/repos", token, "POST", {
            "name": args.repo,
            "private": bool(args.private),
            "description": "投屏隐身提词器 —— 会议共享时稿件窗口不进入画面，跟着说话自动滚动",
            "auto_init": False,   # ★ 必须 false：本地已有内容，初始化会冲突
        })
        if st2 not in (200, 201):
            print("✗ 建仓失败：HTTP %s %s" % (st2, json.dumps(created, ensure_ascii=False)[:300]))
            print("  → 常见原因：token 缺 repo 权限；或该名字已被占用")
            return 5
        print("✓ 已创建：%s" % created["html_url"])
    else:
        print("✗ 查仓库状态失败：HTTP %s %s" % (st, json.dumps(repo, ensure_ascii=False)[:200]))
        return 6

    if args.check:
        print("\n(--check 模式，到此为止)")
        return 0

    if not args.skip_preflight:
        print("\n推送前自检：")
        if not preflight():
            return 7

    url = "https://github.com/%s/%s.git" % (login, args.repo)
    remotes = git("remote", check=False).stdout.split()
    if "origin" in remotes:
        git("remote", "set-url", "origin", url)
        print("\n· 已更新 origin → %s" % url)
    else:
        git("remote", "add", "origin", url)
        print("\n· 已添加 origin → %s" % url)

    branch = args.branch or (git("branch", "--show-current", check=False).stdout.strip() or "master")
    print("· 推送分支 %s …" % branch)

    # ★★ 凭据怎么喂给 git：**一律显式喂"刚验过的那把"**，绝不让 git 自己去读。
    #
    #   为什么不能用 `credential.helper=wincred`：wincred 只会读
    #   `git:https://github.com` 那一条 —— 而**能用的那把可能在 IP 条目里**
    #   （本机实测：github.com 那条 401，140.82.112.4 那条才有效）。
    #   让 git 自己读 ⇒ 又拿到旧 token ⇒ 401，而我们已经验过 token 是好的，
    #   会误判成"推送失败是权限问题"。**验过的必须就是用的那把。**
    #
    #   喂法用**临时凭据文件** + store helper，不用另外两种写法：
    #     - 内联 `!f(){}` helper：会让 git 起一个 sh 子进程，本沙箱直接 SIGTERM；
    #     - `wincred store`：实测**写不进去**（github.com 那条是 GCM 的 wincredman
    #       格式存的，wincred 能读不能写，store 返回 0 但不生效）。
    #   临时文件只含一行，推完 finally 里立刻删，不污染全局凭据。
    #
    # 为什么不走 GCM 的自动登录：本机 GCM 2.9 装了、也能跑（前提 git 在 PATH 里，
    # 否则报 "Failed to locate 'git.exe'"），但设备码/浏览器登录要访问 github.com
    # 主站，在沙箱代理下报「基础连接已经关闭」；而 git 走 libcurl 是通的
    # （实测 ls-remote 返回了 commit hash）⇒ 绕过 GCM 直连更可靠。
    import tempfile
    fd, tmp_cred = tempfile.mkstemp(prefix="prompter-cred-", suffix=".txt")
    os.close(fd)
    with open(tmp_cred, "w", encoding="utf-8") as f:
        f.write("https://x-access-token:%s@github.com\n" % token)
    helper = "store --file=" + tmp_cred.replace("\\", "/")
    print("· 已把验过的 token 写进临时凭据文件（推送后立即删除）")

    try:
        push = subprocess.run(
            [GIT, "-c", "credential.helper=" + helper, "push", "-u", "origin", branch],
            cwd=ROOT, capture_output=True, text=True, encoding="utf-8",
            errors="replace", env=clean_env(),
        )
    finally:
        if tmp_cred and os.path.exists(tmp_cred):
            try:
                os.remove(tmp_cred)
                print("· 临时凭据已删除")
            except OSError:
                print("⚠️ 临时凭据没删掉，请手动删除：%s" % tmp_cred)

    out = (push.stdout or "") + (push.stderr or "")
    print(out.strip()[-1500:] if out.strip() else "(无输出)")
    if push.returncode != 0:
        print("\n✗ 推送失败（rc=%s）" % push.returncode)
        if "Authentication failed" in out or "403" in out or "401" in out:
            print("  → 凭据问题：token 可能过期，或没勾 repo 权限")
        return 8

    print("\n✅ 完成：https://github.com/%s/%s" % (login, args.repo))
    print("   Mac 上：git clone %s" % url)
    return 0


if __name__ == "__main__":
    sys.exit(main())
