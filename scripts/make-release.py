#!/usr/bin/env python3
"""用 GitHub REST API 发布 Release 并上传安装包 —— 不碰 git push。

## 为什么单独一个脚本

`scripts/push-via-api.py` 负责把**代码**推到远端；这个脚本负责把**装出来的包**
挂成 Release 资产，让用户能从

    https://github.com/<owner>/<repo>/releases/latest/download/Prompter_0.1.0_x64-setup.exe

直接下到最新版（`latest/download` 永远指向最新那条 Release，不用改链接）。

## 两条实测结论（2026-10-10）

| 主机 | 结果 |
|---|---|
| `api.github.com` | ✅ 通 |
| `uploads.github.com` | ✅ 通（**资产上传必须走这个域名**，不是 api.github.com） |

资产上传的端点和普通 API 不同：`POST https://uploads.github.com/repos/.../assets`，
且 body 就是**文件的原始字节**（不是 base64、不是 JSON）。

## 幂等

重跑不会炸：同名 tag / 同名 Release / 同名资产都会先查出来复用或替换，
所以「改了包想重发」直接重跑即可。

## 用法

    python scripts/make-release.py --repo prompter --tag v0.1.0 \
        --asset src-tauri/target/release/bundle/nsis/Prompter_0.1.0_x64-setup.exe
    python scripts/make-release.py ... --dry-run     # 只打印计划，不写远端
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys
import time
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))

# 复用 push-to-github.py 的凭据枚举（含 Windows 凭据管理器），避免重复实现。
_spec = importlib.util.spec_from_file_location(
    "ptg", os.path.join(HERE, "push-to-github.py"))
ptg = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(ptg)

ROOT = ptg.ROOT
API = "https://api.github.com"
UPLOADS = "https://uploads.github.com"


def api(token, path, method="GET", payload=None, retries=5):
    """调 api.github.com，对网络异常 / 5xx / 429 做指数退避重试（同 push-via-api.py）。"""
    return _request(token, API + path, method, payload, retries)


def _request(token, url, method="GET", payload=None, retries=5, raw=None,
             content_type=None):
    """通用请求：payload 走 JSON，raw 走原始字节（资产上传用）。"""
    data = None
    if raw is not None:
        data = raw
    elif payload is not None:
        data = json.dumps(payload).encode("utf-8")
    last = (None, {})
    for attempt in range(1, retries + 1):
        req = urllib.request.Request(url, data=data, method=method)
        req.add_header("Authorization", "Bearer " + token)
        req.add_header("Accept", "application/vnd.github+json")
        req.add_header("X-GitHub-Api-Version", "2022-11-28")
        req.add_header("User-Agent", "prompter-release")
        if content_type:
            req.add_header("Content-Type", content_type)
        elif payload is not None:
            req.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(req, timeout=180) as r:
                body = r.read().decode("utf-8", "replace")
                return r.status, (json.loads(body) if body.strip() else {})
        except urllib.error.HTTPError as e:
            text = e.read().decode("utf-8", "replace")
            try:
                parsed = json.loads(text)
            except Exception:
                parsed = {"raw": text[:400]}
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


def login_as(token):
    st, me = api(token, "/user")
    if st != 200:
        return None, None
    return me.get("login"), me


def main():
    ap = argparse.ArgumentParser(description="发 Release + 上传安装包（纯 REST API）")
    ap.add_argument("--repo", default="prompter")
    ap.add_argument("--tag", default="v0.1.0")
    ap.add_argument("--name", default=None, help="Release 标题，默认取 tag")
    ap.add_argument("--asset", required=True, help="要上传的安装包路径")
    ap.add_argument("--notes", default=None, help="Release 说明（markdown 文本文件）")
    ap.add_argument("--target", default=None, help="tag 指向的分支/提交，默认仓库默认分支")
    ap.add_argument("--draft", action="store_true")
    ap.add_argument("--prerelease", action="store_true")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    print("=" * 64)
    print("GitHub Release 发布（api.github.com + uploads.github.com）")
    print("=" * 64)

    asset_path = args.asset if os.path.isabs(args.asset) else os.path.join(ROOT, args.asset)
    if not os.path.isfile(asset_path):
        print("✗ 找不到安装包：%s" % asset_path)
        print("  先出包：npx tauri build --bundles nsis")
        return 2
    asset_name = os.path.basename(asset_path)
    asset_size = os.path.getsize(asset_path)
    print("· 安装包：%s（%.2f MB）" % (asset_name, asset_size / 1024 / 1024))

    # 1) 认证
    token = login = None
    for label, tok, _env in ptg.gather_candidates():
        lg, _me = login_as(tok)
        if lg:
            token, login = tok, lg
            print("✓ 已认证：%s（凭据来源：%s）" % (login, label))
            break
    if not token:
        print("✗ 没有可用凭据")
        return 3

    st, repo = api(token, "/repos/%s/%s" % (login, args.repo))
    if st != 200:
        print("✗ 查仓库失败：HTTP %s %s" % (st, json.dumps(repo)[:200]))
        return 4
    print("✓ 仓库：%s" % repo["html_url"])
    default_branch = args.target or repo.get("default_branch") or "master"

    # 2) 目标提交（tag 要指向的那个 commit）
    st, head = api(token, "/repos/%s/%s/git/ref/heads/%s"
                   % (login, args.repo, default_branch))
    if st == 200:
        target_sha = head["object"]["sha"]
    else:
        # 分支不存在就直接用分支名让 GitHub 自己解析
        target_sha = None
    print("· tag %s → %s（%s）" % (args.tag, default_branch,
                                   (target_sha or "?")[:12]))

    notes = ""
    if args.notes:
        notes_path = args.notes if os.path.isabs(args.notes) else os.path.join(ROOT, args.notes)
        with open(notes_path, "r", encoding="utf-8") as f:
            notes = f.read()

    if args.dry_run:
        print("\n(--dry-run) 将执行：")
        print("  1. 确保 tag %s 存在（指向 %s）" % (args.tag, (target_sha or default_branch)[:12]))
        print("  2. 建/复用 Release「%s」" % (args.name or args.tag))
        print("  3. 上传资产 %s（%.2f MB）" % (asset_name, asset_size / 1024 / 1024))
        print("  4. 直链：https://github.com/%s/%s/releases/latest/download/%s"
              % (login, args.repo, asset_name))
        return 0

    # 3) 确保 tag 存在（不存在就建；已存在则复用，不挪动）
    st, ref = api(token, "/repos/%s/%s/git/ref/tags/%s" % (login, args.repo, args.tag))
    if st == 200:
        print("· tag %s 已存在，复用（%s）" % (args.tag, ref["object"]["sha"][:12]))
    elif st == 404 and target_sha:
        st, made = api(token, "/repos/%s/%s/git/refs" % (login, args.repo), "POST",
                       {"ref": "refs/tags/" + args.tag, "sha": target_sha})
        if st not in (200, 201):
            print("✗ 建 tag 失败：%s" % json.dumps(made, ensure_ascii=False)[:300])
            return 5
        print("✓ 已建 tag %s → %s" % (args.tag, target_sha[:12]))
    else:
        print("· tag 查询 HTTP %s，交由 Release API 自行创建" % st)

    # 4) 建 / 复用 Release
    st, rel = api(token, "/repos/%s/%s/releases/tags/%s" % (login, args.repo, args.tag))
    if st == 200:
        release = rel
        print("· Release %s 已存在，复用（id=%s）" % (args.tag, release["id"]))
    elif st == 404:
        payload = {
            "tag_name": args.tag,
            "name": args.name or args.tag,
            "body": notes,
            "draft": bool(args.draft),
            "prerelease": bool(args.prerelease),
        }
        if target_sha:
            payload["target_commitish"] = target_sha
        st, rel = api(token, "/repos/%s/%s/releases" % (login, args.repo), "POST", payload)
        if st not in (200, 201):
            print("✗ 建 Release 失败：%s" % json.dumps(rel, ensure_ascii=False)[:400])
            return 6
        release = rel
        print("✓ 已建 Release：%s" % release["html_url"])
    else:
        print("✗ 查 Release 失败：HTTP %s %s" % (st, json.dumps(rel)[:200]))
        return 6

    # 5) 上传资产（同名先删后传 —— 保证「重跑 = 换新包」而不是留下旧包）
    st, assets = api(token, "/repos/%s/%s/releases/%s/assets"
                     % (login, args.repo, release["id"]))
    if st == 200:
        for a in assets:
            if a["name"] == asset_name:
                stdel, _ = api(token, "/repos/%s/%s/releases/assets/%s"
                               % (login, args.repo, a["id"]), "DELETE")
                print("· 已删除同名旧资产 %s（HTTP %s）" % (a["name"], stdel))

    with open(asset_path, "rb") as f:
        blob = f.read()
    up_url = "%s/repos/%s/%s/releases/%s/assets?name=%s" % (
        UPLOADS, login, args.repo, release["id"], urllib.request.quote(asset_name))
    print("· 上传中（%.2f MB）…" % (len(blob) / 1024 / 1024))
    st, up = _request(token, up_url, "POST", raw=blob,
                      content_type="application/octet-stream", retries=3)
    if st not in (200, 201):
        print("✗ 上传失败：HTTP %s %s" % (st, json.dumps(up, ensure_ascii=False)[:400]))
        return 7
    print("✓ 已上传：%s（%d 字节）" % (up["name"], up["size"]))

    print()
    print("✅ 完成")
    print("   Release：%s" % release["html_url"])
    print("   直链（永远指向最新版）：")
    print("   https://github.com/%s/%s/releases/latest/download/%s"
          % (login, args.repo, asset_name))
    return 0


if __name__ == "__main__":
    sys.exit(main())
