import sqlite3
import json
import os
import sys
import re
import time
import urllib.parse

SESSION_ID_RE = re.compile(r'([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})', re.IGNORECASE)

def normalize_path(p):
    if not p:
        return ""
    # Strip Win32 long path namespace prefix (e.g. \\?\C:\...)
    p = p.replace('\\\\?\\', '')
    # Normalize slashes to backslashes for uniform comparison on Windows
    p = p.replace('/', '\\')
    return p.lower().strip()

def is_in_any_workspace(cwd, workspace_roots):
    if not cwd or cwd == "outside-of-project":
        return False
    norm_cwd = normalize_path(cwd)
    for root in workspace_roots:
        norm_root = normalize_path(root)
        if not norm_root:
            continue
        if norm_cwd == norm_root or norm_cwd.startswith(norm_root + '\\'):
            return True
    return False

def load_codex_global_state(codex_dir):
    global_state_path = os.path.join(codex_dir, '.codex-global-state.json')
    if not os.path.exists(global_state_path):
        return {}
    try:
        with open(global_state_path, 'r', encoding='utf-8') as f:
            return json.load(f)
    except Exception:
        return {}

def state_value(state, key, default=None):
    if key in state:
        return state.get(key, default)
    persisted = state.get('electron-persisted-atom-state')
    if isinstance(persisted, dict) and key in persisted:
        return persisted.get(key, default)
    return default

def load_session_index(session_index_path):
    names = {}
    updated = {}
    if not os.path.exists(session_index_path):
        return names, updated
    try:
        with open(session_index_path, 'r', encoding='utf-8') as f:
            for line in f:
                try:
                    data = json.loads(line)
                    tid = data.get('id')
                    tname = data.get('thread_name')
                    updated_at = data.get('updated_at')
                    if not tid:
                        continue
                    if tname:
                        names[tid] = tname
                    if updated_at:
                        updated[tid] = updated_at
                except Exception:
                    pass
    except Exception:
        pass
    return names, updated

def collect_ids_from_obj(obj, out):
    if isinstance(obj, dict):
        for key, value in obj.items():
            if isinstance(key, str) and SESSION_ID_RE.fullmatch(key):
                out.add(key)
            collect_ids_from_obj(value, out)
    elif isinstance(obj, list):
        for item in obj:
            if isinstance(item, str) and SESSION_ID_RE.fullmatch(item):
                out.add(item)
            else:
                collect_ids_from_obj(item, out)

def collect_codex_state_thread_ids(state):
    ids = set()
    for key in [
        'unread-thread-ids-by-host-v1',
        'pinned-thread-ids',
        'thread-workspace-root-hints',
    ]:
        collect_ids_from_obj(state_value(state, key, {}), ids)
    return ids

def first_text_from_content(content):
    if isinstance(content, str):
        return content.strip()
    if isinstance(content, list):
        parts = []
        for item in content:
            if not isinstance(item, dict):
                continue
            text = item.get('text') or item.get('input_text') or item.get('output_text')
            if text:
                parts.append(str(text))
        return ' '.join(parts).strip()
    return ''

def title_from_rollout_messages(path):
    try:
        with open(path, 'r', encoding='utf-8', errors='replace') as f:
            for _ in range(80):
                line = f.readline()
                if not line:
                    break
                try:
                    obj = json.loads(line)
                    payload = obj.get('payload', {})
                    if obj.get('type') == 'event_msg' and payload.get('type') == 'user_message':
                        message = str(payload.get('message') or '').strip()
                        if message and not message.startswith('# AGENTS.md instructions'):
                            return message[:80] + ('...' if len(message) > 80 else '')
                    if obj.get('type') == 'response_item' and payload.get('type') == 'message' and payload.get('role') == 'user':
                        text = first_text_from_content(payload.get('content'))
                        if text and not text.startswith('# AGENTS.md instructions') and not text.startswith('<environment_context>'):
                            return text[:80] + ('...' if len(text) > 80 else '')
                except Exception:
                    pass
    except Exception:
        pass
    return 'Codex Chat'

def read_rollout_meta(path):
    try:
        with open(path, 'r', encoding='utf-8', errors='replace') as f:
            first = f.readline()
        data = json.loads(first)
        if data.get('type') != 'session_meta':
            return None
        return data.get('payload', {})
    except Exception:
        return None

def index_rollout_paths(codex_dir):
    by_id = {}
    for subdir, rank in [('sessions', 2), ('archived_sessions', 1)]:
        root_dir = os.path.join(codex_dir, subdir)
        if not os.path.exists(root_dir):
            continue
        for root, _dirs, files in os.walk(root_dir):
            for name in files:
                if not name.startswith('rollout-') or not name.endswith('.jsonl'):
                    continue
                path = os.path.join(root, name)
                match = SESSION_ID_RE.search(name)
                tid = match.group(1) if match else None
                meta = read_rollout_meta(path)
                if meta and meta.get('id'):
                    tid = meta.get('id')
                if not tid:
                    continue
                previous = by_id.get(tid)
                current = {
                    "path": path,
                    "rank": rank,
                    "mtime": os.path.getmtime(path),
                    "meta": meta or {},
                }
                if not previous or (rank, current["mtime"]) >= (previous["rank"], previous["mtime"]):
                    by_id[tid] = current
    return by_id

def codex_row_from_rollout(tid, rollout_info, name_map, updated_map, source_label):
    path = rollout_info.get("path")
    meta = rollout_info.get("meta") or {}
    if meta.get('thread_source') and meta.get('thread_source') != 'user':
        return None
    cwd = meta.get('cwd') or 'outside-of-project'
    return {
        "id": tid,
        "title": name_map.get(tid) or title_from_rollout_messages(path),
        "agent_nickname": "",
        "agent_role": "",
        "cwd": cwd,
        "source": meta.get('source') or source_label,
        "codex_thread_source": meta.get('thread_source') or "user",
        "rollout_path": path,
        "created_at": meta.get('timestamp'),
        "updated_at": updated_map.get(tid),
        "discovery_source": source_label,
    }

def discover_codex_rollout_threads(codex_dir, name_map, updated_map, rollout_index=None):
    rollout_index = rollout_index or index_rollout_paths(codex_dir)
    rows_by_id = {}
    for tid, info in rollout_index.items():
        if info.get("rank") != 2:
            continue
        row = codex_row_from_rollout(tid, info, name_map, updated_map, "rollout")
        if row:
            rows_by_id[tid] = row
    return list(rows_by_id.values())

def discover_codex_state_threads(state, name_map, updated_map, rollout_index):
    rows = []
    workspace_hints = state_value(state, 'thread-workspace-root-hints', {}) or {}
    for tid in collect_codex_state_thread_ids(state):
        info = rollout_index.get(tid)
        if info:
            if info.get("rank") != 2:
                continue
            row = codex_row_from_rollout(tid, info, name_map, updated_map, "codex-state")
        else:
            cwd = workspace_hints.get(tid) or 'outside-of-project'
            row = {
                "id": tid,
                "title": name_map.get(tid) or "Codex Chat",
                "agent_nickname": "",
                "agent_role": "",
                "cwd": cwd,
                "source": "codex-state",
                "codex_thread_source": "user",
                "created_at": None,
                "updated_at": updated_map.get(tid),
                "discovery_source": "codex-state",
            }
        if row:
            rows.append(row)
    return rows

def remote_connection_aliases(state):
    aliases = {}
    for conn in state_value(state, 'codex-managed-remote-connections', []) or []:
        host_id = conn.get('hostId') or ''
        alias = conn.get('alias') or conn.get('displayName') or host_id.replace('remote-ssh-discovered:', '')
        if host_id and alias:
            aliases[host_id] = alias
    return aliases

def infer_route_metadata(cwd, thread_source, state):
    aliases = remote_connection_aliases(state)
    metadata = {
        "nodeName": os.environ.get("CAM_NODE_NAME") or os.environ.get("COMPUTERNAME") or os.environ.get("HOSTNAME") or "local",
        "sourceHost": "local",
        "hostKind": "local",
        "transport": "local",
        "route": "local",
    }
    if thread_source == "antigravity":
        metadata["transport"] = "antigravity"
        metadata["route"] = "antigravity-local"
        return metadata

    normalized = (cwd or "").replace('\\\\?\\', '').replace('\\', '/')
    for project in state_value(state, 'remote-projects', []) or []:
        remote_path = (project.get('remotePath') or '').rstrip('/')
        host_id = project.get('hostId') or ''
        alias = aliases.get(host_id) or host_id.replace('remote-ssh-discovered:', '')
        if remote_path and alias and normalized.startswith(remote_path.rstrip('/') + '/'):
            metadata["nodeName"] = alias
            metadata["sourceHost"] = alias
            metadata["hostKind"] = "remote"
            metadata["transport"] = "codex-managed"
            metadata["route"] = f"codex-managed:{alias}"
            return metadata
    if normalized.startswith('/home/') or normalized.startswith('/root/') or normalized.startswith('/opt/'):
        selected = state_value(state, 'selected-remote-host-id', '') or ''
        alias = aliases.get(selected) or selected.replace('remote-ssh-discovered:', '') or "remote"
        metadata["nodeName"] = alias
        metadata["sourceHost"] = alias
        metadata["hostKind"] = "remote"
        metadata["transport"] = "codex-managed"
        metadata["route"] = f"codex-managed:{alias}"
    return metadata

def decode_varint(buffer, pos):
    val = 0
    shift = 0
    while True:
        b = buffer[pos]
        val |= (b & 0x7f) << shift
        pos += 1
        if not (b & 0x80):
            break
        shift += 7
    return val, pos

def discover_antigravity(workspace_roots, state):
    agy_dir = os.path.expanduser('~/.gemini/antigravity')
    brain_dir = os.path.join(agy_dir, 'brain')
    pb_path = os.path.join(agy_dir, 'agyhub_summaries_proto.pb')
    
    if not os.path.exists(brain_dir):
        return []
        
    # 1. Parse titles from pb file
    titles = {}
    if os.path.exists(pb_path):
        try:
            with open(pb_path, 'rb') as f:
                data = f.read()
            uuid_regex = re.compile(rb'\x0a\x24([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})', re.IGNORECASE)
            for match in uuid_regex.finditer(data):
                uuid_str = match.group(1).decode('utf-8')
                uuid_end = match.end()
                if uuid_end < len(data) and data[uuid_end] == 0x12:
                    submsg_len, pos = decode_varint(data, uuid_end + 1)
                    if pos < len(data) and data[pos] == 0x0a:
                        title_len, title_pos = decode_varint(data, pos + 1)
                        if title_pos + title_len <= len(data):
                            title_bytes = data[title_pos : title_pos + title_len]
                            title_str = title_bytes.decode('utf-8', errors='ignore').strip()
                            if title_str and not any(x in title_str for x in ["{", "}", "\"", ":", "CommandLine", "Cwd"]):
                                titles[uuid_str] = title_str
        except Exception:
            pass

    results = []
    now = time.time()
    
    # 2. Scan folders in brain/
    for d in os.listdir(brain_dir):
        dpath = os.path.join(brain_dir, d)
        if not os.path.isdir(dpath) or len(d) != 36:
            continue
            
        tpath = os.path.join(dpath, ".system_generated", "logs", "transcript.jsonl")
        if not os.path.exists(tpath):
            continue
            
        mtime = os.path.getmtime(tpath)
        
        # Read workspace path from conversations/<uuid>.db
        db_path = os.path.join(agy_dir, 'conversations', f'{d}.db')
        cwd = "outside-of-project"
        if os.path.exists(db_path):
            try:
                # Open read-only connection to avoid database locks
                conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
                cursor = conn.cursor()
                cursor.execute("SELECT name FROM sqlite_master WHERE type='table' AND name='trajectory_metadata_blob'")
                if cursor.fetchone():
                    cursor.execute("SELECT data FROM trajectory_metadata_blob WHERE id='main'")
                    row = cursor.fetchone()
                    if row and row[0]:
                        blob = row[0]
                        paths = re.findall(rb'(?:file:///|[a-zA-Z]:\\)[a-zA-Z0-9_\-\.\:\/\\ \(\)%]+', blob)
                        if paths:
                            decoded = paths[0].decode('utf-8', errors='ignore')
                            if decoded.startswith("file:///"):
                                decoded = decoded[8:]
                            cwd = urllib.parse.unquote(decoded).replace('/', '\\')
                conn.close()
            except Exception:
                pass
                
        # Resolve Title
        title = titles.get(d)
        if not title:
            # Title source 2: Read CONVERSATION_HISTORY from transcript.jsonl
            try:
                with open(tpath, 'r', encoding='utf-8') as f:
                    for _ in range(15):
                        line = f.readline()
                        if not line:
                            break
                        step = json.loads(line)
                        if step.get("type") == "CONVERSATION_HISTORY" and step.get("content"):
                            m = re.search(rf'## Conversation {d}: (.*?)\n', step["content"], re.IGNORECASE)
                            if m:
                                title = m.group(1).strip()
                                break
            except Exception:
                pass
                
        if not title:
            # Title source 3: Read first USER_INPUT
            try:
                with open(tpath, 'r', encoding='utf-8') as f:
                    line = f.readline()
                    if line:
                        step = json.loads(line)
                        if step.get("type") == "USER_INPUT" and step.get("content"):
                            content = re.sub(r'<[^>]+>', '', step["content"]).strip()
                            if content:
                                title = content[:40] + ("..." if len(content) > 40 else "")
            except Exception:
                pass
                
        if not title:
            title = "Antigravity Chat"
            
        in_workspace = is_in_any_workspace(cwd, workspace_roots)
        # We consider active if it matches open workspace, or if it was modified in the last 7 days
        is_active = in_workspace or (now - mtime) < 7 * 86400
        
        if is_active:
            results.append({
                "id": d,
                "title": title,
                "agent_nickname": "",
                "agent_role": "",
                "cwd": cwd,
                "thread_source": "antigravity",
                **infer_route_metadata(cwd, "antigravity", state)
            })
            
    return results

def main():
    try:
        # Resolve the SQLite database path
        db_path = os.environ.get("CODEX_DB_PATH")
        if not db_path:
            db_path = os.path.expanduser('~/.codex/state_5.sqlite')
        
        codex_dir = os.path.dirname(db_path)
        session_index_path = os.path.join(codex_dir, 'session_index.jsonl')
        state = load_codex_global_state(codex_dir)
        
        # 1. Read workspace roots
        workspace_roots = []
        if state:
            try:
                active = state_value(state, 'active-workspace-roots', [])
                saved = state_value(state, 'electron-saved-workspace-roots', [])
                workspace_roots = list(set(active + saved))
            except Exception:
                pass

        # 2. Build thread metadata from session_index.jsonl, keeping the latest seen name.
        name_map, updated_map = load_session_index(session_index_path)

        rows = []
        if os.path.exists(db_path):
            try:
                conn = sqlite3.connect(db_path)
                conn.row_factory = sqlite3.Row
                cursor = conn.cursor()
                
                # Query active user threads
                cursor.execute("""
                    SELECT id, title, agent_nickname, agent_role, cwd, source, thread_source AS codex_thread_source
                    FROM threads 
                    WHERE archived = 0 AND thread_source = 'user'
                """)
                rows = [dict(r) for r in cursor.fetchall()]
                conn.close()
            except Exception:
                pass

        # SQLite is not always updated at the same time as the rollout/session index files.
        # Merge rollout-discovered Codex sessions so a real active transcript is not invisible to CAM.
        seen_codex_ids = set(str(r.get('id')).lower() for r in rows if r.get('id'))
        rollout_index = index_rollout_paths(codex_dir)
        for rollout_row in discover_codex_rollout_threads(codex_dir, name_map, updated_map, rollout_index):
            if str(rollout_row.get('id')).lower() not in seen_codex_ids:
                rows.append(rollout_row)
                seen_codex_ids.add(str(rollout_row.get('id')).lower())
        for state_row in discover_codex_state_threads(state, name_map, updated_map, rollout_index):
            if str(state_row.get('id')).lower() not in seen_codex_ids:
                rows.append(state_row)
                seen_codex_ids.add(str(state_row.get('id')).lower())
        
        # Map custom chat names to titles for Codex threads
        for r in rows:
            latest_name = name_map.get(r['id'])
            if latest_name:
                r['title'] = latest_name
            r['thread_source'] = 'codex'
            r.update(infer_route_metadata(r.get('cwd'), 'codex', state))

        # Discover Antigravity threads
        agy_rows = discover_antigravity(workspace_roots, state)
        rows.extend(agy_rows)
        
        # Filter threads by workspace roots if available
        if workspace_roots:
            filtered_rows = []
            for r in rows:
                # Always include recently active Antigravity threads, or filter Codex threads by workspace
                if r.get('thread_source') == 'antigravity' or is_in_any_workspace(r['cwd'], workspace_roots):
                    filtered_rows.append(r)
            rows = filtered_rows
        
        print(json.dumps({"threads": rows}))
    except Exception as e:
        print(json.dumps({"error": str(e)}))
        sys.exit(1)

if __name__ == '__main__':
    main()
