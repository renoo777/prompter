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

## 两个必须知道的限制（都是实测结论，不是猜的）

### ① 空仓库不能用 Git Data API —— 会 409 "Git Repository is empty."

`POST /git/blobs` 和 `POST /git/trees` 在**没有任何提交**的仓库上会被拒。
处置：先用 Contents API 写一个占位文件把仓库"激活"（产生第一个提交与分支），
再照常重放；完成后把占位分支删掉。脚本已内置这一步。

### ② 重放出来的 commit SHA **不可能**与本地相同 —— 差在消息末尾那个换行

git 的 commit 对象就是一段纯文本：
```
tree <sha>
[parent <sha>]
author <名字> <邮箱> <时间戳> <时区>
committer <名字> <邮箱> <时间戳> <时区>

<消息>
```
其中 `<消息>` 以 `\n` 结尾（git 一贯如此）。而**GitHub 的提交 API 会把消息末尾的换行裁掉**。

实测（根提交，无父提交，排除了父链干扰）：

| 字段 | 本地 | 远端 | 结论 |
|---|---|---|---|
| tree | `a35299c3…` | `a35299c3…` | ✅ **一致** |
| message 长度 | 473 | 472 | 差 1 个字符 |
| 差的字符 | — | — | 就是一个 `\n` |
| 去掉尾部换行后 | 内容完全相同 | | ✅ |

⇒ **内容与权限位逐字节一致（tree SHA 相同即证明），但 commit SHA 必然不同。**
（API 返回的日期显示成 `…Z` 只是 UTC 表示，与本地 `+0800` 是同一时刻，不是差异来源。）

**这个限制没有绕法**：要 SHA 完全相同只能走 `git push`，而 `git push` 走
`github.com:443` —— 那条路在本机网络上不通（见上）。所以：

- 远端 = **内容忠实的一份镜像**；
- 以后网络通了想推本地的新提交，需要 `git push --force`（历史不同源），
  或者干脆以远端为准重新 clone。

---

## 关键设计：逐提交重放（不是把文件堆成一个大提交）

按本地提交顺序逐个重放：每个提交用 `base_tree=父提交的树` + 该提交相对父提交的
**变更集**，并**原样复制 author / committer / 日期 / message**。

好处：远端有**完整的 7 段历史**（而不是一个"导入"提交），每个提交的 tree 都与本地对应提交
**逐字节一致**；脚本会逐个校验 **tree SHA** 并在不一致时当场报出来，不假装成功。

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
import time
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


def api(token, path, method="GET", payload=None, retries=5):
    """调用 GitHub API —— **失败自动重试**。

    ★ 为什么必须重试（2026-10-08 真踩）：一次全量重放要发 ~200 个请求、跑好几分钟，
      中途任何一次 DNS 抖动（实测报 `getaddrinfo failed`）都会让
      **前面积累的成果全部作废**。所以对「网络异常 / 5xx / 429」做指数退避重试；
      4xx 是业务错误（401/404/409/422…），重试没意义，直接返回。
    """
    url = "https://api.github.com" + path
    data = json.dumps(payload).encode("utf-8") if payload is not None else None
    last = (None, {})
    for attempt in range(1, retries + 1):
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
                parsed = json.loads(raw)
            except Exception:
                parsed = {"raw": raw[:400]}
            last = (e.code, parsed)
            if e.code < 500 and e.code != 429:
                return last
        except Exception as e:
            last = (None, {"error": "%s: %s" % (type(e).__name__, e)})
        if attempt < retries:
            wait = min(2 ** attempt, 20)
            print("    · 网络/服务异常，%ds 后重试（%d/%d）：%s"
                  % (wait, attempt, retries, str(last[1])[:90]))
            time.sleep(wait)
    return last


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
    if len(cache) % 40 == 0:
        print("    · 已上传 %d 个 blob…" % len(cache))
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

    # ★ 空仓库不能直接建 blob/tree：GitHub 会返回 409 "Git Repository is empty."
    #   （Git Data API 的已知限制，不是权限问题）。
    #   处置：先用 Contents API 写一个占位文件把仓库"激活"（产生第一个提交和分支），
    #   之后照常重放本地提交 —— 我们的根提交本来就没有父提交，
    #   所以重放出来的 SHA 仍与本地逐字节一致；最后把占位用的分支删掉。
    st_refs, refs = api(token, "/repos/%s/%s/git/refs" % (login, args.repo))
    bootstrap_branch = None
    if st_refs != 200 or not refs:
        dflt = repo.get("default_branch") or "main"
        print("· 仓库为空 ⇒ 先用 Contents API 激活（临时分支 %s，完成后删除）" % dflt)
        st, boot = api(token, "/repos/%s/%s/contents/.prompter-bootstrap"
                       % (login, args.repo), "PUT", {
                           "message": "chore: 初始化仓库（占位提交，随后由真实历史替换）",
                           "content": base64.b64encode(
                               b"Temporary bootstrap commit for Git Data API.\n").decode(),
                           "branch": dflt,
                       })
        if st not in (200, 201):
            print("✗ 激活空仓库失败：%s" % json.dumps(boot, ensure_ascii=False)[:300])
            return 4
        bootstrap_branch = dflt

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
        # ★ 真正该校验的是 **tree**，不是 commit SHA：
        #   commit SHA 必然不同（GitHub 会裁掉消息末尾换行，见文件头说明），
        #   而 tree SHA 相同 == 该提交的文件内容与权限位逐字节一致。
        tree_ok = (remote_tree == m["tree"])
        print("  [%d/%d] %s  %s  tree %s" % (
            idx, len(commits), c[:12], m["message"].splitlines()[0][:32],
            "✓ 一致" if tree_ok else "✗ 不一致！本地 %s" % m["tree"][:12]))
        if not tree_ok:
            print("✗ tree 不一致 ⇒ 内容迁移有误，中止（远端已被写脏，需重建）")
            return 6

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

    # 清理激活用的占位分支：先把默认分支切到真实分支，再删占位分支。
    if bootstrap_branch and bootstrap_branch != args.branch:
        st, _ = api(token, "/repos/%s/%s" % (login, args.repo), "PATCH",
                    {"default_branch": args.branch})
        if st == 200:
            print("· 默认分支已切到 %s" % args.branch)
        st, delb = api(token, "/repos/%s/%s/git/refs/heads/%s"
                       % (login, args.repo, bootstrap_branch), "DELETE")
        if st in (200, 204):
            print("· 已删除占位分支 %s（其提交变为不可达，不影响仓库）" % bootstrap_branch)
        else:
            print("⚠️ 占位分支 %s 没删掉（HTTP %s）—— 不影响代码，可手动删"
                  % (bootstrap_branch, st))

    # 5) 内容核验 + 说明本地与远端的关系
    local_head = git_text("rev-parse", "HEAD").strip()
    local_tree = git_text("rev-parse", "HEAD^{tree}").strip()
    print()
    if local_tree != remote_tree:
        print("✗ 内容核验失败：tree 不同（本地 %s / 远端 %s）"
              % (local_tree[:12], remote_tree[:12]))
        return 8
    print("✓ 内容核验：远端 HEAD tree == 本地 HEAD tree（%s）" % local_tree[:12])
    print("  ⇒ 全部文件内容与权限位逐字节一致。")
    print("· commit SHA 必然不同（GitHub 提交 API 裁掉消息末尾换行）⇒ 本地与远端是")
    print("  「内容相同、历史不同源」。将来要推本地新提交，需 `git push --force`，")
    print("  或以远端为准重新 clone。")
    print("  本地 HEAD %s ／ 远端 HEAD %s" % (local_head[:12], remote_commit[:12]))

    print("\n✅ 完成：%s" % repo["html_url"])
    print("   Mac 上（github.com 通则）：git clone https://github.com/%s/%s.git" % (login, args.repo))
    print("   网络不通时：用 _transfer/prompter.bundle")
    return 0


if __name__ == "__main__":
    sys.exit(main())
