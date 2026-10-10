#!/usr/bin/env python3
"""用 GitHub REST API 发布 Release、上传安装包、并维护应用内更新的清单 —— 不碰 git push。

## 为什么单独一个脚本

`scripts/push-via-api.py` 负责把**代码**推到远端；这个脚本负责把**装出来的包**
挂成 Release 资产，让用户能从

    https://github.com/<owner>/<repo>/releases/latest/download/Prompter_0.1.0_x64-setup.exe

直接下到最新版（`latest/download` 永远指向最新那条 Release，不用改链接）。

它还负责 `latest.json` —— **应用内更新的更新清单**。客户端装在同事机器上之后，
每隔一段时间去这个固定地址问一次"有没有新版本"：

    https://github.com/<owner>/<repo>/releases/latest/download/latest.json

⇒ 所以每次发版都必须把 `latest.json` 重新传一遍，否则同事那边**永远收不到更新通知**
（而且不报错，表现为"检查更新：已是最新"）。

## 两条实测结论（2026-10-10）

| 主机 | 结果 |
|---|---|
| `api.github.com` | ✅ 通 |
| `uploads.github.com` | ✅ 通（**资产上传必须走这个域名**，不是 api.github.com） |

资产上传的端点和普通 API 不同：`POST https://uploads.github.com/repos/.../assets`，
且 body 就是**文件的原始字节**（不是 base64、不是 JSON）。

## 应用内更新：三件东西必须一起传（2026-10-10 加）

出包时打开 `bundle.createUpdaterArtifacts` 之后，`npx tauri build` 会**额外**
产出一样东西：更新包的签名。

    Prompter_0.1.1_x64-setup.exe        ← 安装包**本身就是**更新包（见下）
    Prompter_0.1.1_x64-setup.exe.sig    ← 它的签名（minisign）

配上 `latest.json` 里的那一条，三样缺一不可：

    latest.json 指到 .exe 的下载地址  +  .exe 本体  +  .sig 里的签名

### ★★ Windows 上「更新包」不是单独的 `.nsis.zip`

很多教程（和 Tauri v1 / 早期 v2）说 NSIS 的产物是
`Prompter_0.1.1_x64-setup.nsis.zip` + `.nsis.zip.sig`。
**本机实测（tauri-cli 2.12.2）不是这样**：`--bundles nsis` 只多出一个
`-setup.exe.sig`，全盘找不到任何 `.nsis.zip`。

    Finished 1 updater signature at:
        src-tauri/target/release/bundle/nsis/Prompter_0.1.1_x64-setup.exe.sig

`.sig` 里的 trusted comment 也直接点名了它签的是谁：
`file:Prompter_0.1.1_x64-setup.exe	version:0.1.1`。
官方文档 Building 一节同口径 ——「`myapp-setup.exe` … it will be re-used
by the updater」。⇒ **不要去找 `.nsis.zip`，找不到就等于功能没做**
（你会一直以为打包失败）。macOS 那边才是单列的 `.app.tar.gz` + `.app.tar.gz.sig`。

⇒ 于是 Windows 上传时**同一个 `.exe` 会被用两次**：一次当"给人下载的安装包"，
一次当"给 updater 下载的更新包"。脚本对同名的第二次上传做了跳过（见 6b），
否则 7MB 要白传第二遍。

★ **验签是强制的**：`.exe` 与 `.sig` 对不上（或者公钥换了）⇒ 更新装不上。
  Tauri 刻意不提供"跳过验签"的开关。

★ 清单是**多平台合并**的：`.platforms` 下按 `windows-x86_64` /
  `darwin-aarch64` / `darwin-x86_64` 分键。Windows 与 macOS 要分别在各自的
  机器上出包，所以这个脚本**读回已有的 latest.json 再合并**，而不是覆盖 ——
  否则在 Mac 上发一次就把 Windows 那条抹掉了（反过来也一样），
  而客户端那边表现为"这台机器检查不到更新"。

## 幂等

重跑不会炸：同名 tag / 同名 Release / 同名资产都会先查出来复用或替换，
所以「改了包想重发」直接重跑即可。

## 用法

    # 只挂安装包（不带应用内更新）
    python scripts/make-release.py --repo prompter --tag v0.1.1 \
        --asset src-tauri/target/release/bundle/nsis/Prompter_0.1.1_x64-setup.exe

    # Windows：安装包 + 更新包 + 清单
    # ★ 注意 `--asset` 与 `--updater-artifact` 指的是**同一个 .exe** ——
    #   Windows 的更新包就是安装包本身（见上）。第二次上传会被自动跳过。
    python scripts/make-release.py --repo prompter --tag v0.1.1 \
        --asset src-tauri/target/release/bundle/nsis/Prompter_0.1.1_x64-setup.exe \
        --updater-artifact src-tauri/target/release/bundle/nsis/Prompter_0.1.1_x64-setup.exe \
        --updater-target windows-x86_64 \
        --notes docs/release-notes-v0.1.1.md

    # macOS（在 Mac 上跑；target 换成 darwin-aarch64）
    python scripts/make-release.py --repo prompter --tag v0.1.1 \
        --updater-artifact src-tauri/target/release/bundle/macos/Prompter.app.tar.gz \
        --updater-target darwin-aarch64

    python scripts/make-release.py ... --dry-run     # 只打印计划，不写远端
"""
from __future__ import annotations

import argparse
import datetime
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

# `latest.json` 里 `platforms` 允许的键（Tauri 的 target triple 口径）。
UPDATER_TARGETS = ("windows-x86_64", "darwin-aarch64", "darwin-x86_64",
                   "linux-x86_64")


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


def abs_path(p):
    return p if os.path.isabs(p) else os.path.join(ROOT, p)


def release_assets(token, login, repo, release_id):
    """列出某条 Release 已有的资产。拿不到就当空表（后续上传仍会走同名删除）。"""
    st, assets = api(token, "/repos/%s/%s/releases/%s/assets"
                     % (login, repo, release_id))
    return assets if st == 200 and isinstance(assets, list) else []


def drop_asset_by_name(token, login, repo, assets, name):
    """同名资产先删 —— 保证「重跑 = 换新文件」而不是留下旧的。"""
    for a in assets:
        if a.get("name") == name:
            st, _ = api(token, "/repos/%s/%s/releases/assets/%s"
                        % (login, repo, a["id"]), "DELETE")
            print("· 已删除同名旧资产 %s（HTTP %s）" % (a["name"], st))
            return True
    return False


def upload_asset(token, login, repo, release_id, path, name=None):
    """上传一个文件作为 Release 资产。返回 (ok, 说明)。"""
    name = name or os.path.basename(path)
    with open(path, "rb") as f:
        blob = f.read()
    up_url = "%s/repos/%s/%s/releases/%s/assets?name=%s" % (
        UPLOADS, login, repo, release_id, urllib.request.quote(name))
    print("· 上传 %s（%.2f MB）…" % (name, len(blob) / 1024 / 1024))
    st, up = _request(token, up_url, "POST", raw=blob,
                      content_type="application/octet-stream", retries=3)
    if st not in (200, 201):
        return False, "HTTP %s %s" % (st, json.dumps(up, ensure_ascii=False)[:300])
    return True, "%s（%d 字节）" % (up.get("name"), up.get("size"))


def fetch_asset_bytes(token, login, repo, asset_id):
    """取资产内容（原始字节）。

    ★ `Accept` 必须是 `application/octet-stream` —— 默认那个
      `application/vnd.github+json` 拿到的是**资产的元数据 JSON**，
      不是文件内容。少了这一行，读回来的 `latest.json` 会是一个
      描述它自己的 JSON 对象（`{"name": ..., "size": ...}`），
      然后被当成清单解析、静默生成一份错的。
    """
    url = "%s/repos/%s/%s/releases/assets/%s" % (API, login, repo, asset_id)
    req = urllib.request.Request(url, method="GET")
    req.add_header("Authorization", "Bearer " + token)
    req.add_header("Accept", "application/octet-stream")
    req.add_header("X-GitHub-Api-Version", "2022-11-28")
    req.add_header("User-Agent", "prompter-release")
    try:
        with urllib.request.urlopen(req, timeout=180) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()
    except Exception as e:
        return None, str(e).encode()


def main():
    ap = argparse.ArgumentParser(
        description="发 Release + 上传安装包/更新包 + 维护 latest.json（纯 REST API）")
    ap.add_argument("--repo", default="prompter")
    ap.add_argument("--tag", default="v0.1.1")
    ap.add_argument("--name", default=None, help="Release 标题，默认取 tag")
    ap.add_argument("--asset", action="append", default=[],
                    help="要上传的普通资产（安装包等）。可重复传多次")
    ap.add_argument("--notes", default=None, help="Release 说明（markdown 文本文件）")
    ap.add_argument("--target", default=None, help="tag 指向的分支/提交，默认仓库默认分支")

    # ---- 应用内更新（可选）----
    ap.add_argument("--updater-artifact", default=None,
                    help="更新包本体（Windows: 就是 -setup.exe 本身，不是 .nsis.zip；"
                         "macOS: *.app.tar.gz）。同目录下必须有同名 + '.sig' 的签名文件")
    ap.add_argument("--updater-target", default=None,
                    choices=UPDATER_TARGETS,
                    help="这个更新包属于哪个平台（写进 latest.json 的 platforms 键）")
    ap.add_argument("--updater-version", default=None,
                    help="写进 latest.json 的版本号（不带 v）。默认从 tag 去掉前缀 v")
    ap.add_argument("--updater-notes", default=None,
                    help="写进 latest.json 的更新说明；默认截取 Release 说明前 1500 字")

    ap.add_argument("--draft", action="store_true")
    ap.add_argument("--prerelease", action="store_true")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    if not args.asset and not args.updater_artifact:
        print("✗ 至少要给一样：--asset（安装包）或 --updater-artifact（更新包）")
        return 2

    print("=" * 64)
    print("GitHub Release 发布（api.github.com + uploads.github.com）")
    print("=" * 64)

    # ---- 0) 先把本地文件都核一遍，再动远端 ----
    assets: list[str] = []
    for p in args.asset:
        ap_ = abs_path(p)
        if not os.path.isfile(ap_):
            print("✗ 找不到安装包：%s" % ap_)
            print("  先出包：npx tauri build --bundles nsis app")
            return 2
        assets.append(ap_)
        print("· 安装包：%s（%.2f MB）"
              % (os.path.basename(ap_), os.path.getsize(ap_) / 1024 / 1024))

    updater = None
    if args.updater_artifact:
        if not args.updater_target:
            print("✗ 给了 --updater-artifact 就必须给 --updater-target"
                  "（%s 之一）" % " / ".join(UPDATER_TARGETS))
            return 2
        zip_path = abs_path(args.updater_artifact)
        sig_path = zip_path + ".sig"
        if not os.path.isfile(zip_path):
            print("✗ 找不到更新包：%s" % zip_path)
            print("  出包前确认 tauri.conf.json 里有 bundle.createUpdaterArtifacts: true，")
            print("  且构建时带了 TAURI_SIGNING_PRIVATE_KEY_PATH。")
            return 2
        if not os.path.isfile(sig_path):
            # ★ 没有 .sig ⇒ 更新**一定装不上**（验签是强制的）。在这里就拦住，
            #   别等同事那头报"更新失败"。
            print("✗ 找不到签名文件：%s" % sig_path)
            print("  它由构建时自动生成；没有它说明那次构建没带签名私钥。")
            return 2
        with open(sig_path, "r", encoding="utf-8") as f:
            signature = f.read().strip()
        version = args.updater_version or args.tag.lstrip("v")
        updater = {
            "zip": zip_path,
            "zip_name": os.path.basename(zip_path),
            "sig_name": os.path.basename(sig_path),
            "target": args.updater_target,
            "version": version,
            "signature": signature,
        }
        print("· 更新包：%s（%.2f MB）→ %s，版本 %s"
              % (updater["zip_name"], os.path.getsize(zip_path) / 1024 / 1024,
                 updater["target"], version))

    # ---- 1) 认证 ----
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

    # ---- 2) 目标提交（tag 要指向的那个 commit）----
    st, head = api(token, "/repos/%s/%s/git/ref/heads/%s"
                   % (login, args.repo, default_branch))
    target_sha = head["object"]["sha"] if st == 200 else None
    print("· tag %s → %s（%s）" % (args.tag, default_branch,
                                   (target_sha or "?")[:12]))

    notes = ""
    if args.notes:
        with open(abs_path(args.notes), "r", encoding="utf-8") as f:
            notes = f.read()

    direct = "https://github.com/%s/%s/releases/latest/download/%s"

    if args.dry_run:
        print("\n(--dry-run) 将执行：")
        print("  1. 确保 tag %s 存在（指向 %s）" % (args.tag, (target_sha or default_branch)[:12]))
        print("  2. 建/复用 Release「%s」" % (args.name or args.tag))
        for p in assets:
            print("  3. 上传资产 %s（%.2f MB）"
                  % (os.path.basename(p), os.path.getsize(p) / 1024 / 1024))
        if updater:
            print("  4. 上传更新包 %s + %s" % (updater["zip_name"], updater["sig_name"]))
            print("  5. 合并并上传 latest.json（platforms.%s = %s）"
                  % (updater["target"], updater["version"]))
            print("     直链：https://github.com/%s/%s/releases/download/%s/%s"
                  % (login, args.repo, args.tag, updater["zip_name"]))
        if assets:
            print("  6. 安装包直链：%s"
                  % (direct % (login, args.repo, os.path.basename(assets[0]))))
        print("  7. 清单直链：%s" % (direct % (login, args.repo, "latest.json")))
        return 0

    # ---- 3) 确保 tag 存在（不存在就建；已存在则复用，不挪动）----
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

    # ---- 4) 建 / 复用 Release ----
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

    existing = release_assets(token, login, args.repo, release["id"])

    # ---- 5) 上传普通资产 ----
    #    记下传过谁：Windows 上「安装包」与「更新包」是**同一个 .exe**，
    #    第 6b 步靠这个集合避免把它再传一遍（白传 7MB）。
    uploaded: set[str] = set()
    for p in assets:
        name = os.path.basename(p)
        drop_asset_by_name(token, login, args.repo, existing, name)
        ok, info = upload_asset(token, login, args.repo, release["id"], p)
        if not ok:
            print("✗ 上传失败：%s" % info)
            return 7
        uploaded.add(name)
        print("✓ 已上传：%s" % info)

    # ---- 6) 更新包 + latest.json ----
    if updater:
        # 6a) 先读回已有的 latest.json（**多平台合并**的关键一步）。
        #     Windows 与 macOS 分别在各自机器上出包，谁后发都不能抹掉对方那条。
        manifest = None
        old = next((a for a in existing if a.get("name") == "latest.json"), None)
        if old:
            st, blob = fetch_asset_bytes(token, login, args.repo, old["id"])
            if st == 200:
                try:
                    manifest = json.loads(blob.decode("utf-8"))
                    print("· 已读回现有 latest.json（含 %d 个平台条目：%s）"
                          % (len(manifest.get("platforms") or {}),
                             ", ".join((manifest.get("platforms") or {}).keys()) or "无"))
                except Exception as e:
                    print("· 现有 latest.json 解析失败（%s），重新生成" % e)
            else:
                print("· 现有 latest.json 取不下来（HTTP %s），重新生成" % st)

        platforms = dict((manifest or {}).get("platforms") or {})
        zip_url = "https://github.com/%s/%s/releases/download/%s/%s" % (
            login, args.repo, args.tag, updater["zip_name"])
        platforms[updater["target"]] = {
            "signature": updater["signature"],
            "url": zip_url,
        }

        note_text = args.updater_notes
        if note_text is None:
            note_text = notes[:1500]

        manifest = {
            "version": updater["version"],
            "notes": note_text,
            "pub_date": datetime.datetime.now(datetime.timezone.utc)
            .strftime("%Y-%m-%dT%H:%M:%SZ"),
            "platforms": platforms,
        }
        body = json.dumps(manifest, ensure_ascii=False, indent=2).encode("utf-8")

        # 6b) 更新包本体 + 签名（两个都要，缺一个客户端就装不上）
        for path in (updater["zip"], updater["zip"] + ".sig"):
            name = os.path.basename(path)
            if name in uploaded:
                # Windows 走这条：同一个 -setup.exe 在第 5 步已经当安装包传过。
                # 再传一遍内容逐字节相同，只是白花一次 7MB 上行。
                print("· %s 已在第 5 步作为安装包传过，跳过（同一个文件）" % name)
                continue
            drop_asset_by_name(token, login, args.repo, existing, name)
            ok, info = upload_asset(token, login, args.repo, release["id"], path)
            if not ok:
                print("✗ 上传失败：%s" % info)
                return 7
            print("✓ 已上传：%s" % info)

        # 6c) 清单最后传 —— 它是**入口**：它一到，客户端下次检查就会看到新版本。
        #     先传它、后传本体的话，存在一个"清单已更新、包还没上去"的窗口，
        #     那段时间下载会 404。
        drop_asset_by_name(token, login, args.repo, existing, "latest.json")
        up_url = "%s/repos/%s/%s/releases/%s/assets?name=latest.json" % (
            UPLOADS, login, args.repo, release["id"])
        print("· 上传 latest.json（platforms: %s）…" % ", ".join(platforms.keys()))
        st, up = _request(token, up_url, "POST", raw=body,
                          content_type="application/octet-stream", retries=3)
        if st not in (200, 201):
            print("✗ 上传 latest.json 失败：HTTP %s %s"
                  % (st, json.dumps(up, ensure_ascii=False)[:300]))
            return 7
        print("✓ 已上传：%s（%d 字节）" % (up.get("name"), up.get("size")))

    print()
    print("✅ 完成")
    print("   Release：%s" % release["html_url"])
    if assets:
        print("   安装包（永远指向最新版）：")
        print("   " + direct % (login, args.repo, os.path.basename(assets[0])))
    if updater:
        print("   更新清单（客户端轮的就是这个地址）：")
        print("   " + direct % (login, args.repo, "latest.json"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
