#!/usr/bin/env python3
"""Drive a running GPUI Studio through the real gpui-mcp server.

This exercises the same path a coding agent (Claude Code, Codex, ...) uses, so
the person ↔ agent workflow can be tested without a model:

    # 1. Run Studio with the agent bridge (the default), then:
    python3 scripts/mcp_pipeline.py smoke           # one pass over the commands
    python3 scripts/mcp_pipeline.py agent --once    # scripted collaborator

`agent` waits for the person's chat message, shows its status on the canvas,
makes a visible edit on the layers the person attached, and replies. Use
`--server PATH` when `gpui-mcp` is not on PATH. Exits non-zero on failure.
Only the Python standard library is needed.
"""

import argparse
import json
import subprocess
import sys
import time


class Mcp:
    """Minimal MCP stdio client (JSON-RPC, newline-delimited)."""

    def __init__(self, server):
        self.proc = subprocess.Popen(
            [server],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=sys.stderr,
            text=True,
        )
        self.next_id = 0
        self.rpc(
            "initialize",
            {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "gpui-studio-pipeline", "version": "1"},
            },
        )
        self.notify("notifications/initialized")

    def notify(self, method, params=None):
        message = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            message["params"] = params
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()

    def rpc(self, method, params=None):
        self.next_id += 1
        message = {"jsonrpc": "2.0", "id": self.next_id, "method": method}
        if params is not None:
            message["params"] = params
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()
        while True:
            line = self.proc.stdout.readline()
            if not line:
                raise SystemExit("gpui-mcp exited (is Studio running with the bridge?)")
            reply = json.loads(line)
            if reply.get("id") == self.next_id:
                if "error" in reply:
                    raise RuntimeError(f"{method}: {reply['error']}")
                return reply["result"]

    def tool(self, name, arguments=None):
        result = self.rpc("tools/call", {"name": name, "arguments": arguments or {}})
        if result.get("isError"):
            raise RuntimeError(f"{name}: {result['content'][0].get('text')}")
        if result.get("structuredContent") is not None:
            return result["structuredContent"]
        text = result["content"][0].get("text", "")
        try:
            return json.loads(text)
        except ValueError:
            return text

    def command(self, name, arguments=None):
        out = self.tool("execute_app_command", {"name": name, "arguments": arguments or {}})
        return out.get("result", {}).get("output", out) if isinstance(out, dict) else out

    def close(self):
        self.proc.stdin.close()
        self.proc.wait(timeout=10)


def check(label, condition):
    print(("ok   " if condition else "FAIL ") + label)
    if not condition:
        raise SystemExit(1)


def smoke(mcp):
    apps = mcp.tool("list_apps")
    check("Studio is connected", any(a["app_id"] == "gpui-studio" for a in apps["apps"]))
    names = {c["name"] for c in mcp.tool("list_app_commands")["commands"]}
    for required in ["get_document", "read_messages", "send_message", "set_status", "export_image"]:
        check(f"command {required}", required in names)
    doc = mcp.command("get_document")
    board = doc["pages"][0]["artboards"][0]["id"]
    tree = mcp.command("get_tree", {"node_id": board, "depth": 2})
    check("layer tree", bool(tree.get("children")))
    found = mcp.tool("find_elements", {"query": doc["pages"][0]["artboards"][0]["name"]})
    check("layers are findable by name", found["count"] > 0)
    resources = {r["uri"] for r in mcp.rpc("resources/list", {})["resources"]}
    check("chat resource", "gpui-studio://chat" in resources)
    check("live document", "html" in json.dumps(mcp.tool("get_live_document")))
    mcp.command("set_status", {"agent": "Pipeline", "status": "Smoke test", "node_ids": [board]})
    mcp.command("set_status", {"done": True})
    print("smoke: all checks passed")


def agent(mcp, once, timeout):
    print("waiting for the person to send a chat message (⌘J in Studio)…")
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            mcp.tool(
                "wait_for_element",
                {"query": "Unread chat messages for the agent", "timeout_ms": 30000},
            )
        except RuntimeError:
            continue
        inbox = mcp.command("read_messages", {"agent": "Pipeline agent"})
        for message in inbox["messages"]:
            if message["from"] != "person":
                continue
            layers = [layer["id"] for layer in message["layers"]]
            if not layers:
                layers = [mcp.command("get_document")["pages"][0]["artboards"][0]["id"]]
            mcp.command("set_status", {"status": f"Working on: {message['text'][:40]}", "node_ids": layers})
            if inbox.get("paused"):
                mcp.command("send_message", {"text": "Edits are paused — I'll wait.", "reply_to": message["id"]})
                continue
            time.sleep(0.5)
            mcp.command("update_styles", {"node_ids": layers, "styles": {"border": "2px dashed #f97316"}})
            mcp.command(
                "send_message",
                {
                    "text": "Marked the layers you attached with a dashed outline (scripted agent).",
                    "reply_to": message["id"],
                    "node_ids": layers,
                },
            )
            mcp.command("set_status", {"done": True})
            print(f"answered message {message['id']}: {message['text']!r}")
            if once:
                return
    raise SystemExit("timed out waiting for a chat message")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("mode", choices=["smoke", "agent"])
    parser.add_argument("--server", default="gpui-mcp", help="path to the gpui-mcp server binary")
    parser.add_argument("--once", action="store_true", help="agent: stop after one reply")
    parser.add_argument("--timeout", type=float, default=600, help="agent: give up after N seconds")
    args = parser.parse_args()
    mcp = Mcp(args.server)
    try:
        if args.mode == "smoke":
            smoke(mcp)
        else:
            agent(mcp, args.once, args.timeout)
    finally:
        mcp.close()


if __name__ == "__main__":
    main()
