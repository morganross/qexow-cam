import sqlite3
import json
import os
import sys

def normalize_path(p):
    if not p:
        return ""
    # Strip Win32 long path namespace prefix (e.g. \\?\C:\...)
    p = p.replace('\\\\?\\', '')
    # Normalize slashes to backslashes for uniform comparison on Windows
    p = p.replace('/', '\\')
    return p.lower().strip()

def is_in_any_workspace(cwd, workspace_roots):
    if not cwd:
        return False
    norm_cwd = normalize_path(cwd)
    for root in workspace_roots:
        norm_root = normalize_path(root)
        if not norm_root:
            continue
        if norm_cwd == norm_root or norm_cwd.startswith(norm_root + '\\'):
            return True
    return False

def main():
    try:
        # Resolve the SQLite database path
        db_path = os.environ.get("CODEX_DB_PATH")
        if not db_path:
            db_path = os.path.expanduser('~/.codex/state_5.sqlite')
        
        if not os.path.exists(db_path):
            print(json.dumps({"error": f"Database not found at {db_path}"}))
            sys.exit(1)
            
        codex_dir = os.path.dirname(db_path)
        global_state_path = os.path.join(codex_dir, '.codex-global-state.json')
        session_index_path = os.path.join(codex_dir, 'session_index.jsonl')
        
        # 1. Read workspace roots
        workspace_roots = []
        if os.path.exists(global_state_path):
            try:
                with open(global_state_path, 'r', encoding='utf-8') as f:
                    state = json.load(f)
                active = state.get('active-workspace-roots', [])
                saved = state.get('electron-saved-workspace-roots', [])
                workspace_roots = list(set(active + saved))
            except Exception as e:
                pass

        # 2. Build thread_name map from session_index.jsonl (keeping the latest name)
        name_map = {}
        if os.path.exists(session_index_path):
            try:
                with open(session_index_path, 'r', encoding='utf-8') as f:
                    for line in f:
                        try:
                            data = json.loads(line)
                            tid = data.get('id')
                            tname = data.get('thread_name')
                            if tid and tname:
                                name_map[tid] = tname
                        except Exception:
                            pass
            except Exception:
                pass

        conn = sqlite3.connect(db_path)
        conn.row_factory = sqlite3.Row
        cursor = conn.cursor()
        
        # Query active user threads
        cursor.execute("""
            SELECT id, title, agent_nickname, agent_role, cwd 
            FROM threads 
            WHERE archived = 0 AND thread_source = 'user'
        """)
        rows = [dict(r) for r in cursor.fetchall()]
        
        # Filter by desktop workspace roots if available
        if workspace_roots:
            filtered_rows = []
            for r in rows:
                if is_in_any_workspace(r['cwd'], workspace_roots):
                    filtered_rows.append(r)
            rows = filtered_rows
        
        # Map custom chat names to titles
        for r in rows:
            latest_name = name_map.get(r['id'])
            if latest_name:
                r['title'] = latest_name
        
        print(json.dumps({"threads": rows}))
    except Exception as e:
        print(json.dumps({"error": str(e)}))
        sys.exit(1)

if __name__ == '__main__':
    main()
