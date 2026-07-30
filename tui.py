#!/usr/bin/env python3
"""Interactive Terminal User Interface (TUI) & Tester for Freemodel API Proxy."""

import sys
import os
import time
import json
import subprocess
import urllib.request
import urllib.error
import httpx
from rich.console import Console
from rich.panel import Panel
from rich.prompt import Prompt, Confirm
from rich.table import Table
from rich.text import Text
from rich.markdown import Markdown
from rich.layout import Layout
from rich.align import Align

import config

console = Console()

PROXY_URL = f"http://{config.DEFAULT_HOST}:{config.DEFAULT_PORT}"
if config.DEFAULT_HOST == "0.0.0.0":
    PROXY_URL = f"http://127.0.0.1:{config.DEFAULT_PORT}"

def check_proxy_online() -> bool:
    try:
        req = urllib.request.Request(f"{PROXY_URL}/health", headers={"User-Agent": "WorkBuddy/0.1.0"})
        with urllib.request.urlopen(req, timeout=2) as resp:
            return resp.status == 200
    except Exception:
        return False

def get_current_key() -> str:
    return os.environ.get("FREEMODEL_API_KEY") or config.load_saved_key()

def start_proxy_background():
    if check_proxy_online():
        console.print(f"[bold yellow]Proxy server is already running on {PROXY_URL}![/bold yellow]")
        return
    console.print(f"[bold cyan]Starting Freemodel Proxy Server in background on {PROXY_URL}...[/bold cyan]")
    script_dir = os.path.dirname(os.path.abspath(__file__))
    log_file = open(os.path.join(script_dir, "proxy_server.log"), "a")
    proc = subprocess.Popen(
        [sys.executable, "-m", "uvicorn", "proxy_server:app", "--host", config.DEFAULT_HOST, "--port", str(config.DEFAULT_PORT)],
        cwd=script_dir,
        stdout=log_file,
        stderr=log_file
    )
    time.sleep(2)
    if check_proxy_online():
        console.print("[bold green]✓ Proxy server started successfully![/bold green]")
    else:
        console.print("[bold red]⚠ Server process launched, waiting for port...[/bold red]")

def render_banner():
    console.clear()
    online = check_proxy_online()
    key = get_current_key()
    
    status_str = "[bold green]ONLINE[/bold green]" if online else "[bold red]OFFLINE[/bold red]"
    if key:
        masked_key = key[:4] + "••••" + key[-4:] if len(key) > 8 else "••••••••"
        key_str = f"[bold green]Configured[/bold green] ({masked_key})"
    else:
        key_str = "[bold red]NOT SET[/bold red]"

    grid = Table.grid(expand=True)
    grid.add_column(justify="left")
    grid.add_column(justify="right")
    grid.add_row(
        "[bold magenta]⚡ FREEMODEL PROXY TUI & TESTER[/bold magenta]",
        f"Server: {status_str}  |  API Key: {key_str}"
    )

    console.print(Panel(grid, border_style="cyan", padding=(0, 1)))

def set_api_key_prompt():
    render_banner()
    console.print(Panel("[bold yellow]Set / Change Freemodel API Key[/bold yellow]", border_style="yellow"))
    current = get_current_key()
    if current:
        console.print(f"Current Key: [dim]{current[:4]}...{current[-4:] if len(current)>8 else ''}[/dim]")
    
    new_key = Prompt.ask("[bold green]Enter your Freemodel API Key[/bold green]", password=True)
    if new_key.strip():
        config.save_key(new_key.strip())
        config.DEFAULT_API_KEY = new_key.strip()
        os.environ["FREEMODEL_API_KEY"] = new_key.strip()
        console.print("[bold green]✓ API Key saved successfully to config.json![/bold green]")
    else:
        console.print("[yellow]No key entered. Unchanged.[/yellow]")
    Prompt.ask("\nPress [bold cyan]Enter[/bold cyan] to return to menu...")

def list_models_and_status():
    render_banner()
    console.print(Panel("[bold cyan]System Status & Available Models[/bold cyan]", border_style="cyan"))
    
    if not check_proxy_online():
        console.print("[bold red]Proxy server is currently OFFLINE.[/bold red]")
        if Confirm.ask("Would you like to start the proxy server now?"):
            start_proxy_background()
    
    if check_proxy_online():
        try:
            client = httpx.Client(timeout=5.0)
            res = client.get(f"{PROXY_URL}/v1/models")
            if res.status_code == 200:
                data = res.json()
                models = data.get("data", [])
                table = Table(title="Available Models in Proxy", header_style="bold magenta")
                table.add_column("Model ID", style="cyan")
                table.add_column("Object", style="dim")
                table.add_column("Owned By", style="green")
                
                for m in models:
                    table.add_row(m.get("id", ""), m.get("object", ""), m.get("owned_by", ""))
                
                console.print(table)
            else:
                console.print(f"[red]Error fetching models: HTTP {res.status_code}[/red]")
        except Exception as e:
            console.print(f"[red]Failed to connect to proxy: {e}[/red]")
    
    Prompt.ask("\nPress [bold cyan]Enter[/bold cyan] to return to menu...")

def choose_project() -> str:
    default = os.path.abspath(config.PROXY_DEFAULT_PROJECT)
    while True:
        project = Prompt.ask("[bold cyan]Project directory[/bold cyan]", default=default).strip()
        resolved = os.path.abspath(os.path.expanduser(project))
        if os.path.isdir(resolved):
            return resolved
        console.print(f"[red]Directory does not exist: {resolved}[/red]")


def choose_proxy_session(project: str) -> dict:
    with httpx.Client(timeout=10.0) as client:
        response = client.get(f"{PROXY_URL}/proxy/sessions", params={"project": project})
        response.raise_for_status()
        sessions = response.json().get("data", [])
        if sessions:
            table = Table(title="Proxy sessions for this project")
            table.add_column("Choice", style="cyan")
            table.add_column("Title")
            table.add_column("Session ID", style="dim")
            table.add_column("Last used")
            table.add_row("0", "Create new session", "", "")
            for index, session in enumerate(sessions, 1):
                table.add_row(str(index), session["title"], session["id"], session["updated_at"])
            console.print(table)
            choices = [str(index) for index in range(len(sessions) + 1)]
            selected = Prompt.ask("Select session", choices=choices, default="0")
            if selected != "0":
                return sessions[int(selected) - 1]
        title = Prompt.ask("New session title", default=os.path.basename(project) or "Proxy session")
        response = client.post(f"{PROXY_URL}/proxy/sessions", json={"project": project, "title": title})
        response.raise_for_status()
        return response.json()


def save_history(session_id: str, messages: list[dict]):
    with httpx.Client(timeout=10.0) as client:
        response = client.post(
            f"{PROXY_URL}/proxy/sessions/{session_id}/history",
            json={"messages": messages},
        )
        response.raise_for_status()


def interactive_chat():
    render_banner()
    if not check_proxy_online():
        console.print("[bold yellow]Proxy server is offline. Starting it now...[/bold yellow]")
        start_proxy_background()
        if not check_proxy_online():
            console.print("[bold red]Could not connect to proxy server at " + PROXY_URL + "[/bold red]")
            Prompt.ask("\nPress Enter to return...")
            return

    key = get_current_key()
    if not key:
        console.print("[bold yellow]⚠ Warning: No API Key configured. Requests will rely on proxy defaults.[/bold yellow]")

    project = choose_project()
    session = choose_proxy_session(project)
    session_id = session["id"]
    model = "gpt-5.6-sol"
    console.print(
        Panel(
            f"[bold green]Interactive Proxy Session[/bold green]\n"
            f"Project: [cyan]{project}[/cyan]\n"
            f"Session: [cyan]{session['title']}[/cyan] [dim]({session_id})[/dim]\n"
            f"Model: [cyan]{model}[/cyan] | URL: [dim]{PROXY_URL}/v1/chat/completions[/dim]\n"
            "Type [bold red]'exit'[/bold red] or [bold red]'quit'[/bold red] to return to menu.",
            border_style="green",
        )
    )

    history = list(session.get("history") or [])
    if history:
        console.print(f"[dim]Restored {len(history)} saved messages.[/dim]")
    
    while True:
        try:
            user_input = Prompt.ask("\n[bold cyan]You[/bold cyan]")
            if not user_input.strip():
                continue
            if user_input.strip().lower() in ["exit", "quit", ":q"]:
                break
                
            history.append({"role": "user", "content": user_input})
            
            headers = {
                "Content-Type": "application/json",
                "X-WorkBuddy-Session": session_id,
                "X-WorkBuddy-Project": project,
            }
            if key:
                headers["Authorization"] = f"Bearer {key}"
                
            payload = {
                "model": model,
                "messages": history,
                "stream": True
            }
            
            console.print(f"\n[bold magenta]Assistant ({model})[/bold magenta]:", end=" ")
            
            assistant_reply = ""
            try:
                with httpx.Client(timeout=60.0) as client:
                    with client.stream("POST", f"{PROXY_URL}/v1/chat/completions", json=payload, headers=headers) as response:
                        if response.status_code == 200:
                            for chunk in response.iter_text():
                                for line in chunk.split("\n"):
                                    line = line.strip()
                                    if line.startswith("data: ") and line != "data: [DONE]":
                                        try:
                                            data_json = json.loads(line[6:])
                                            choices = data_json.get("choices", [])
                                            if choices:
                                                delta = choices[0].get("delta", {})
                                                content = delta.get("content", "")
                                                if content:
                                                    console.print(content, end="")
                                                    assistant_reply += content
                                        except Exception:
                                            pass
                            console.print()
                        else:
                            console.print(f"[bold red]\nHTTP Error {response.status_code}: {response.text}[/bold red]")
                            assistant_reply = f"[Error HTTP {response.status_code}]"
            except Exception as e:
                console.print(f"\n[bold red]Request Error: {e}[/bold red]")
                assistant_reply = f"[Request Error: {e}]"
                
            if assistant_reply and not assistant_reply.startswith("["):
                assistant_message = {"role": "assistant", "content": assistant_reply}
                history.append(assistant_message)
                try:
                    save_history(session_id, [history[-2], assistant_message])
                except Exception as exc:
                    console.print(f"[yellow]Could not persist session history: {exc}[/yellow]")
                
        except KeyboardInterrupt:
            console.print("\n[yellow]Chat session interrupted.[/yellow]")
            break

def main_menu():
    while True:
        render_banner()
        table = Table(show_header=False, box=None, padding=(0, 2))
        table.add_column("Option", style="bold cyan")
        table.add_column("Description")
        
        table.add_row("[1]", "💬 Test Chat & Send Prompt (Interactive)")
        table.add_row("[2]", "🔑 Set / Update Freemodel API Key")
        table.add_row("[3]", "📋 View Models & Server Health")
        table.add_row("[4]", "🚀 Start Proxy Server (Background)")
        table.add_row("[0]", "❌ Exit TUI")
        
        console.print(Panel(table, title="[bold]Main Menu[/bold]", border_style="magenta"))
        
        choice = Prompt.ask("[bold green]Select an option[/bold green]", choices=["0", "1", "2", "3", "4"], default="1")
        
        if choice == "1":
            interactive_chat()
        elif choice == "2":
            set_api_key_prompt()
        elif choice == "3":
            list_models_and_status()
        elif choice == "4":
            start_proxy_background()
            Prompt.ask("\nPress [bold cyan]Enter[/bold cyan] to return to menu...")
        elif choice == "0":
            console.print("[bold cyan]Goodbye![/bold cyan]")
            break

if __name__ == "__main__":
    if len(sys.argv) > 1:
        if sys.argv[1] == "--key" and len(sys.argv) > 2:
            key_val = sys.argv[2]
            config.save_key(key_val)
            print(f"API Key saved: {key_val[:4]}...{key_val[-4:] if len(key_val)>8 else ''}")
            sys.exit(0)
        elif sys.argv[1] == "--start":
            start_proxy_background()
            sys.exit(0)
            
    main_menu()
