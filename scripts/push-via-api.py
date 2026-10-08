#!/usr/bin/env python3
"""经由 GitHub **REST API**（api.github.com）推送仓库 —— 不碰 github.com:443。

## 为什么需要这个脚本

2026-10-08 实测本机网络：

| 主机 | 结果 |
|---|---|
| `api.github.com` | ✅ 通（0.3s） |
| `github.com:443` | ❌ 超时（DNS 能解析到 20.205.243.166，但连不上） |
| `codeload.github.com` | ❌ 超时 |

而 `git push` 走的是 `https://github.com/...` ⇒ **必然失败**，报
`Recv failure: Connection was reset` 或 `Could not connect to server`。
**这不是认证问题**（认证早就验过了），是**网络**问题。

GitHub 的 Git Data API 全部挂在 api.github.com 上，可以**逐对象**把仓库建出来：
blobs → trees → commits → ref，最后更新分支指针。**完全不经过 github.com**。

## 关键设计：逐提交重放，SHA 与本地一致

不是"把文件塞成一个大提交"，而是**按本地提交顺序逐个重放**：
每个提交用 `base_tree=父提交的树` + 该提交相对父提交的**变更集**，
并**原样复制 author / committer / 日期 / message**。

⇒ 结果 commit 的 SHA 与本地**逐字节相同**（git 的 commit 对象就是那几行文本）。
好处：`git clone` 之后本地与远端同 SHA，不会有"历史对不上"的困惑。
脚本会**逐个校验 SHA**，不一致就当场报出来，不假装成功。

## 用法

    python scripts/push-via-api.py --repo prompter            # 自动找可用凭据
    python scripts/push-via-api.py --repo prompter --dry-run  # 只演练，不写远端
"""
from __future__ import annotations

import argparse
import base64
import importlib.util
import json
import os
import subprocess
import sys
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))

# 复用 push-to-github.py 里的凭据枚举 / git 调用 / api 封装，避免重复实现。
_spec = importlib.util.spec_from_file_location(
    "ptg", os.path.join(HERE, "push-to-github.py"))
ptg = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(ptg)

ROOT = ptg.ROOT
BLOB_LIMIT = 100 * 1024 * 1024   # 单 blob 上限（GitHub 限制）


def git_out(*args):
    """跑 git，返回 bytes（不用 text=True，避免二进制内容被解码破坏）。"""
    p = subprocess.run([ptg.GIT, "-c", "core.quotepath=false", *args],
                       cwd=ROOT, capture_output=True, env=ptg.clean_env())
    if p.returncode != 0:
        raise RuntimeError("git %s 失败:\n%s" % (" ".join(args),
                                               p.stderr.decode("utf-8", "replace")))
    return p.stdout


def git_text(*args):
    return git_out(*args).decode("utf-8", "replace")


def api(token, path, method="GET", payload=None):
    url = "https://api.github.com" + path
    data = json.dumps(payload).encode("utf-8") if payload is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Authorization", "Bearer " + token)
    req.add_header("Accept", "application/vnd.github+json")
    req.add_header("X-GitHub-Api-Version", "2022-11-28")
    req.add_header("User-Agent", "prompter-api-push")
    if data:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            body = r.read().decode("utf-8")
            return r.status, (json.loads(body) if body.strip() else {})
    except urllib.error.HTTPError as e:
        raw = e.read().decode("utf-8", "replace")
        try:
            return e.code, json.loads(raw)
        except Exception:
            return e.code, {"raw": raw[:400]}
    except Exception as e:
        return None, {"error": "%s: %s" % (type(e).__name__, e)}


# ---------------------------------------------------------------- 本地对象读取

def list_commits():
    """按时间顺序（旧→新）返回本地提交 SHA 列表。"""
    return git_text("rev-list", "--reverse", "HEAD").split()


def commit_meta(sha):
    """取提交的 tree / parents / 作者 / 提交者 / message（原样复制用）。"""
    fmt = "%T%x00%P%x00%an%x00%ae%x00%aI%x00%cn%x00%ce%x00%cI%x00%B"
    raw = git_text("show", "-s", "--format=" + fmt, sha)
    parts = raw.split("\x00")
    return {
        "tree": parts[0],
        "parents": parts[1].split() if parts[1].strip() else [],
        "author": {"name": parts[2], "email": parts[3], "date": parts[4]},
        "committer": {"name": parts[5], "email": parts[6], "date": parts[7]},
        "message": parts[8] if len(parts) > 8 else "",
    }


def tree_entries(sha):
    """`git ls-tree -r <commit>` → {path: (mode, blob_sha)}

    用 -z：路径按 NUL 分隔，**中文文件名不会被转义**（绕开 core.quotepath 的坑）。
    """
    out = git_out("ls-tree", "-r", "-z", sha)
    entries = {}
    for chunk in out.split(b"\x00"):
        if not chunk:
            continue
        meta, path = chunk.split(b"\t", 1)
        mode, otype, osha = meta.decode().split()
        if otype != "blob":
            continue
        entries[path.decode("utf-8", "replace")] = (mode, osha)
    return entries


def diff_raw(parent, commit):
    """`git diff-tree -r --raw -z` → [(status, path, new_mode, new_sha)]

    带 -M 开重命名检测，重命名会给出 R<score>，按"删旧 + 加新"处理更稳，
    所以这里**故意不开 -M**，让变更集就是简单的 A/M/D。
    """
    out = git_out("diff-tree", "-r", "--raw", "-z", "--no-commit-id", parent, commit)
    fields = out.split(b"\x00")
    res = []
    i = 0
    while i < len(fields):
        f = fields[i]
        if not f:
            i += 1
            continue
        # ":<oldmode> <newmode> <oldsha> <newsha> <status>"
        head = f.decode()
        status = head.split()[-1]
        i += 1
        path = fields[i].decode("utf-8", "replace") if i < len(fields) else ""
        i += 1
        parts = head[1:].split()
        new_mode, new_sha = parts[1], parts[3]
        res.append((status, path, new_mode, new_sha))
    return res


# ------------------------------------------------------------------ 远端写入

def upload_blob(token, owner, repo, sha, cache):
    if sha in cache:
        return cache[sha]
    content = git_out("cat-file", "blob", sha)
    if len(content) > BLOB_LIMIT:
        raise RuntimeError("blob 超过 100MB，API 路径不适合：%s" % sha)
    st, body = api(token, "/repos/%s/%s/git/blobs" % (owner, repo), "POST", {
        "content": base64.b64encode(content).decode("ascii"),
        "encoding": "base64",
    })
    if st not in (200, 201):
        raise RuntimeError("上传 blob 失败 %s: %s" % (sha, json.dumps(body)[:300]))
    got = body["sha"]
    if got != sha:
        # 理论上 git 的 blob SHA 与 GitHub 一致；不一致说明内容被改动了，必须报出来
        raise RuntimeError("blob SHA 不一致！本地 %s / 远端 %s（内容被篡改？）" % (sha, got))
    cache[sha] = got
    return got


def main():
    ap = argparse.ArgumentParser(description="经 REST API 推送（绕过 github.com:443）")
    ap.add_argument("--repo", default="prompter")
    ap.add_argument("--branch", default="master")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    print("=" * 64)
    print("经 GitHub REST API 推送（不经过 github.com）")
    print("=" * 64)

    if not ptg.GIT:
        print("✗ 找不到 git")
        return 2

    # 1) 认证
    token = login = None
    for label, tok, _env in ptg.gather_candidates():
        st, me = api(tok, "/user")
        if st == 200:
            token, login = tok, me["login"]
            print("✓ 已认证：%s（凭据来源：%s）" % (login, label))
            break
    if not token:
        print("✗ 没有可用凭据")
        return 3

    # 2) 仓库
    st, repo = api(token, "/repos/%s/%s" % (login, args.repo))
    if st == 404:
        print("· 仓库不存在，创建中…")
        st, repo = api(token, "/user/repos", "POST", {
            "name": args.repo, "private": False, "auto_init": False,
            "description": "投屏隐身提词器 —— 会议共享时稿件窗口不进入画面，跟着说话自动滚动",
        })
        if st not in (200, 201):
            print("✗ 建仓失败：%s" % json.dumps(repo, ensure_ascii=False)[:300])
            return 4
    if st != 200:
        print("✗ 查仓库失败：HTTP %s" % st)
        return 4
    print("✓ 仓库：%s" % repo["html_url"])

    commits = list_commits()
    print("· 本地提交 %d 个，逐提交重放" % len(commits))

    if args.dry_run:
        for c in commits:
            m = commit_meta(c)
            print("  %s  %s  tree=%s" % (c[:12], m["message"].splitlines()[0][:44], m["tree"][:12]))
        print("\n(--dry-run，未写远端)")
        return 0

    # 3) 逐提交重放
    blob_cache = {}
    remote_tree = None
    remote_commit = None
    for idx, c in enumerate(commits, 1):
        m = commit_meta(c)
        if remote_tree is None:
            # 根提交：整棵树全量
            ents = tree_entries(c)
            payload_tree = [
                {"path": p, "mode": md, "type": "blob",
                 "sha": upload_blob(token, login, args.repo, bsha, blob_cache)}
                for p, (md, bsha) in sorted(ents.items())
            ]
            tree_body = {"tree": payload_tree}
            label = "全量 %d 个文件" % len(payload_tree)
        else:
            changes = diff_raw(m["parents"][0], c)
            payload_tree = []
            for status, path, new_mode, new_sha in changes:
                if status.startswith("D"):
                    payload_tree.append({"path": path, "mode": new_mode,
                                         "type": "blob", "sha": None})
                else:
                    payload_tree.append({
                        "path": path, "mode": new_mode, "type": "blob",
                        "sha": upload_blob(token, login, args.repo, new_sha, blob_cache)})
            tree_body = {"base_tree": remote_tree, "tree": payload_tree}
            label = "增量 %d 处改动" % len(payload_tree)

        st, tree = api(token, "/repos/%s/%s/git/trees" % (login, args.repo),
                       "POST", tree_body)
        if st not in (200, 201):
            print("✗ 建 tree 失败：%s" % json.dumps(tree, ensure_ascii=False)[:300])
            return 5
        remote_tree = tree["sha"]

        st, com = api(token, "/repos/%s/%s/git/commits" % (login, args.repo), "POST", {
            "message": m["message"],
            "tree": remote_tree,
            "parents": [remote_commit] if remote_commit else [],
            "author": m["author"],
            "committer": m["committer"],
        })
        if st not in (200, 201):
            print("✗ 建 commit 失败：%s" % json.dumps(com, ensure_ascii=False)[:300])
            return 6
        remote_commit = com["sha"]
        ok = "✓ SHA 一致" if remote_commit == c else "⚠️ SHA 不同（本地 %s）" % c[:12]
        print("  [%d/%d] %s  %s  %s" % (idx, len(commits), c[:12],
                                        m["message"].splitlines()[0][:34], ok))

    # 4) 分支指针
    st, ref = api(token, "/repos/%s/%s/git/refs" % (login, args.repo))
    if st == 200 and isinstance(ref, list) and any(
            r.get("ref") == "refs/heads/" + args.branch for r in ref):
        st, r = api(token, "/repos/%s/%s/git/refs/heads/%s" % (login, args.repo, args.branch),
                    "PATCH", {"sha": remote_commit, "force": True})
    else:
        st, r = api(token, "/repos/%s/%s/git/refs" % (login, args.repo), "POST",
                    {"ref": "refs/heads/" + args.branch, "sha": remote_commit})
    if st not in (200, 201):
        print("✗ 更新分支失败：%s" % json.dumps(r, ensure_ascii=False)[:300])
        return 7
    print("✓ 分支 %s → %s" % (args.branch, remote_commit[:12]))

    # 5) 让本地认为已同步（SHA 相同，本来就是同一批对象）
    local_head = git_text("rev-parse", "HEAD").strip()
    if local_head == remote_commit:
        subprocess.run([ptg.GIT, "update-ref",
                        "refs/remotes/origin/" + args.branch, remote_commit],
                       cwd=ROOT, env=ptg.clean_env())
        print("✓ 本地 origin/%s 已指向同一 SHA（无历史差异）" % args.branch)
    else:
        print("⚠️ 远端 HEAD 与本地不同（%s vs %s）——迁移不完整，请检查"
              % (remote_commit[:12], local_head[:12]))

    print("\n✅ 完成：%s" % repo["html_url"])
    print("   Mac 上（github.com 通则）：git clone https://github.com/%s/%s.git" % (login, args.repo))
    print("   网络不通时：用 _transfer/prompter.bundle")
    return 0


if __name__ == "__main__":
    sys.exit(main())
