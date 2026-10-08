#!/usr/bin/env python3
"""把当前仓库推送到 GitHub —— 认证 → 建仓 → 关联 → 推送，一条命令走完。

为什么需要这个脚本（而不是手敲三条 git 命令）：
  1. 这一步最容易卡在「远端仓库还没建」——脚本会用 API 自动建，省掉去网页点一遍。
  2. 建仓 + 关联 + 推送之间有依赖（必须先有仓库才能 add remote），
     手敲容易在中途出错后留下半截状态（remote 加了但仓库没建，再跑就报
     "repository not found"，看着像权限问题，其实是顺序问题）。
  3. 顺带做推送前的自检：把不该上传的东西（target/、node_modules/）挡在推送前，
     而不是等 GitHub 拒收或传了 18GB 才发现。

-token 的来源（按优先级）：
  1. 环境变量 GITHUB_TOKEN
  2. Windows 凭据管理器（git-credential-wincred 里 host=github.com 那条）
  **token 只在内存里用，不落盘、不打印。**

用法：
    python scripts/push-to-github.py --repo prompter            # 自动读凭据
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
WINCRED_CANDIDATES = [
    r"C:\Users\26611\.workbuddy\binaries\PortableGit\versions\1.2.0\mingw64\bin\git-credential-wincred.exe",
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
WINCRED = find_first(WINCRED_CANDIDATES)


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


def token_from_wincred():
    if not WINCRED or not os.path.exists(WINCRED):
        return None
    p = subprocess.run([WINCRED, "get"], input="protocol=https\nhost=github.com\n\n",
                       capture_output=True, text=True, encoding="utf-8",
                       errors="replace", env=clean_env(), timeout=25)
    for line in (p.stdout or "").splitlines():
        if line.startswith("password="):
            return line[len("password="):].strip()
    return None


def get_token():
    tok = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    if tok:
        return tok.strip(), "环境变量"
    tok = token_from_wincred()
    if tok:
        return tok, "Windows 凭据管理器"
    return None, None


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

    token, src = get_token()
    if not token:
        print("✗ 没有可用凭据。")
        print("  → 设 GITHUB_TOKEN 环境变量，或往 Windows 凭据管理器里加一条")
        print("    git:https://github.com （用户名填 x-access-token，密码填 PAT）")
        return 3
    print("token 来源：%s" % src)

    st, me = api("/user", token)
    if st != 200:
        print("✗ 凭据无效：HTTP %s %s" % (st, me.get("message", "")))
        print("  → token 过期或被撤销了，需要一把新的（Settings → Developer settings"
              " → Personal access tokens → 勾 repo 权限）")
        return 4
    login = me["login"]
    print("✓ 已认证：%s" % login)

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

    # ★ 走 wincred helper，让 git 自己取凭据 —— token 不进命令行、不进历史
    push = subprocess.run(
        [GIT, "-c", "credential.helper=wincred", "push", "-u", "origin", branch],
        cwd=ROOT, capture_output=True, text=True, encoding="utf-8",
        errors="replace", env=clean_env(),
    )
    out = (push.stdout or "") + (push.stderr or "")
    print(out.strip()[-1500:] if out.strip() else "(无输出)")
    if push.returncode != 0:
        print("\n✗ 推送失败（rc=%s）" % push.returncode)
        return 8

    print("\n✅ 完成：https://github.com/%s/%s" % (login, args.repo))
    print("   Mac 上：git clone %s" % url)
    return 0


if __name__ == "__main__":
    sys.exit(main())
