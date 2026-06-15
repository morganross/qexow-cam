import React, { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import {
  Activity,
  CircleAlert,
  Eye,
  Inbox,
  ListRestart,
  MessageSquareText,
  Play,
  Radar,
  RefreshCcw,
  RotateCw,
  Search,
  Send,
  Square,
  Users
} from 'lucide-react';
import './styles.css';

function App() {
  const [home, setHome] = useState('');
  const [status, setStatus] = useState(null);
  const [agents, setAgents] = useState([]);
  const [peers, setPeers] = useState([]);
  const [inbox, setInbox] = useState(null);
  const [logs, setLogs] = useState(null);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [lastSendResult, setLastSendResult] = useState(null);
  const [operationResult, setOperationResult] = useState(null);
  const [agentFilter, setAgentFilter] = useState({
    text: '',
    kind: 'all',
    route: 'all',
    status: 'all',
    chatStatus: 'all'
  });
  const [sendForm, setSendForm] = useState({
    target_agent: '',
    body: '',
    source_agent: '',
    correlation_id: '',
    message_type: '',
    strict: true
  });
  const [agentForm, setAgentForm] = useState({ name: '', source: 'codex', cwd: '', thread_id: '' });

  async function refresh() {
    setBusy(true);
    setError('');
    try {
      const [homeValue, statusValue, agentsValue, inboxValue, logsValue, peersValue] = await Promise.allSettled([
        window.cam.home(),
        window.cam.status(),
        window.cam.api({ path: '/v1/agents', method: 'GET' }),
        window.cam.api({ path: '/v1/inbox', method: 'GET' }),
        window.cam.api({ path: '/v1/logs', method: 'GET' }),
        window.cam.api({ path: '/v1/peers', method: 'GET' })
      ]);
      if (homeValue.status === 'fulfilled') setHome(homeValue.value);
      if (statusValue.status === 'fulfilled') setStatus(statusValue.value);
      if (agentsValue.status === 'fulfilled') setAgents(asArray(agentsValue.value.body));
      if (inboxValue.status === 'fulfilled') setInbox(inboxValue.value.body);
      if (logsValue.status === 'fulfilled') setLogs(logsValue.value.body);
      if (peersValue.status === 'fulfilled') setPeers(asArray(peersValue.value.body));
      const failure = [homeValue, statusValue, agentsValue, inboxValue, logsValue, peersValue].find(
        (result) => result.status === 'rejected'
      );
      if (failure) setError(failure.reason.message);
      const apiFailure = [agentsValue, inboxValue, logsValue, peersValue].find(
        (result) => result.status === 'fulfilled' && result.value && result.value.ok === false
      );
      if (apiFailure) setError(textOf(apiFailure.value.body));
    } catch (caught) {
      setError(caught.message);
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => {
    refresh();
    const id = setInterval(refresh, 5000);
    return () => clearInterval(id);
  }, []);

  async function daemonCommand(args, success) {
    setBusy(true);
    setError('');
    setNotice('');
    try {
      const result = await window.cam.daemonCommand(args);
      if (!result.ok) throw new Error(result.stderr || result.stdout || result.error || 'command failed');
      setNotice(success);
      await refresh();
    } catch (caught) {
      setError(caught.message);
    } finally {
      setBusy(false);
    }
  }

  async function createAgent(event) {
    event.preventDefault();
    setBusy(true);
    setError('');
    setNotice('');
    try {
      const body = Object.fromEntries(Object.entries(agentForm).filter(([, value]) => value.trim()));
      const response = await window.cam.api({ path: '/v1/agents', method: 'POST', body });
      if (!response.ok) throw new Error(textOf(response.body));
      setNotice(`Agent ${agentForm.name} created`);
      setAgentForm({ name: '', source: 'codex', cwd: '', thread_id: '' });
      await refresh();
    } catch (caught) {
      setError(caught.message);
    } finally {
      setBusy(false);
    }
  }

  async function runOperation(label, operation) {
    setBusy(true);
    setError('');
    setNotice('');
    setOperationResult(null);
    try {
      const result = await operation();
      setOperationResult({ label, result });
      setNotice(operationNotice(label, result));
      await refresh();
    } catch (caught) {
      setError(caught.message);
    } finally {
      setBusy(false);
    }
  }

  async function runDiscovery() {
    return runOperation('Local discovery', async () => {
      const response = await window.cam.api({
        path: '/v1/discovery/local:run',
        method: 'POST',
        body: { promote_approved: true }
      });
      if (!response.ok) throw new Error(textOf(response.body));
      return response.body;
    });
  }

  async function syncPeers() {
    return runOperation('Peer sync', async () => {
      const response = await window.cam.api({ path: '/v1/peers:sync', method: 'POST' });
      if (!response.ok) throw new Error(textOf(response.body));
      return response.body;
    });
  }

  async function resumeAgent(agentName) {
    return runOperation(`Resume ${agentName}`, async () => {
      const response = await window.cam.api({ path: `/v1/agents/${agentName}/resume`, method: 'POST' });
      if (!response.ok) throw new Error(textOf(response.body));
      return response.body;
    });
  }

  async function readAgent(agentName) {
    return runOperation(`Read ${agentName}`, async () => {
      const response = await window.cam.api({
        path: `/v1/agents/${agentName}/thread?latest=true&include_turns=true&turns=5`,
        method: 'GET'
      });
      if (!response.ok) throw new Error(textOf(response.body));
      return response.body;
    });
  }

  async function sendMessage(event) {
    event.preventDefault();
    setBusy(true);
    setError('');
    setNotice('');
    setLastSendResult(null);
    try {
      const body = {
        target_agent: sendForm.target_agent.trim(),
        message: sendForm.body.trim(),
        strict: sendForm.strict
      };
      for (const field of ['source_agent', 'correlation_id', 'message_type']) {
        const value = sendForm[field].trim();
        if (value) body[field] = value;
      }
      const response = await window.cam.api({ path: '/v1/messages', method: 'POST', body });
      if (!response.ok) throw new Error(textOf(response.body));
      setLastSendResult(response.body);
      setNotice(sendNotice(response.body));
      setSendForm({ ...sendForm, body: '' });
      await refresh();
    } catch (caught) {
      setError(caught.message);
    } finally {
      setBusy(false);
    }
  }

  const daemon = status?.health?.daemon || status?.daemon;
  const running = Boolean(status?.health?.daemon?.running || status?.daemon?.observed_state === 'running');
  const safeAgents = asArray(agents);
  const safePeers = asArray(peers);
  const filteredAgents = safeAgents.filter((agent) => agentMatchesFilter(agent, agentFilter));
  const agentCounts = summarizeAgents(safeAgents);

  return (
    <main className="shell">
      <section className="topbar">
        <div>
          <p className="eyebrow">Qexow CAM Desktop</p>
          <h1>Agent traffic control</h1>
        </div>
        <div className="actions">
          <button onClick={refresh} disabled={busy}><RefreshCcw size={18} />Refresh</button>
          <button onClick={runDiscovery} disabled={busy}><Radar size={18} />Discover</button>
          <button onClick={syncPeers} disabled={busy}><RotateCw size={18} />Sync Peers</button>
          <button onClick={() => daemonCommand(['init'], 'CAM home initialized')} disabled={busy}><ListRestart size={18} />Init</button>
          <button onClick={() => daemonCommand(['daemon', 'start', '--headless'], 'Daemon started')} disabled={busy}><Play size={18} />Start</button>
          <button onClick={() => daemonCommand(['daemon', 'stop'], 'Daemon stopped')} disabled={busy}><Square size={18} />Stop</button>
        </div>
      </section>

      {(error || notice || status?.error) && (
        <section className={`banner ${error || status?.error ? 'bad' : 'good'}`}>
          <CircleAlert size={18} />
          <span>{error || status?.error || notice}</span>
        </section>
      )}

      <section className="status-grid">
        <Metric icon={<Activity />} label="Daemon" value={running ? 'running' : 'not running'} detail={daemon?.startup_phase || 'unknown'} />
        <Metric icon={<Users />} label="Agents" value={String(safeAgents.length)} detail={home || 'home unknown'} />
        <Metric icon={<RotateCw />} label="Peers" value={String(safePeers.length)} detail={peerHealth(safePeers)} />
        <Metric icon={<Inbox />} label="Inbox" value={String(inbox?.messages?.length ?? 0)} detail={inbox?.timed_out ? 'wait timed out' : 'current mailbox'} />
      </section>

      <section className="panel health-panel">
        <header><Activity size={18} /><h2>System Health</h2></header>
        <dl className="summary-grid">
          <dt>CAM home</dt><dd>{home || 'unknown'}</dd>
          <dt>Daemon state</dt><dd>{status?.daemon ? `${status.daemon.bind || '127.0.0.1'}:${status.daemon.port || 'unknown'} pid ${status.daemon.pid || 'unknown'}` : 'not readable'}</dd>
          <dt>Observed state</dt><dd>{status?.daemon?.observed_state || status?.health?.daemon?.observed_state || 'unknown'}</dd>
          <dt>Health endpoint</dt><dd>{status?.health ? 'responding' : status?.error || 'not responding'}</dd>
          <dt>Last heartbeat</dt><dd>{status?.daemon?.last_heartbeat_at || status?.health?.daemon?.last_heartbeat_at || 'unknown'}</dd>
          <dt>Recent log rows</dt><dd>{Array.isArray(logs) ? logs.length : logs?.events?.length ?? logs?.entries?.length ?? 'unknown'}</dd>
        </dl>
      </section>

      <section className="workspace">
        <div className="panel agents">
          <header><Users size={18} /><h2>Agents</h2></header>
          <div className="agent-tools">
            <label className="search-field">
              <Search size={16} />
              <input
                value={agentFilter.text}
                onChange={(event) => setAgentFilter({ ...agentFilter, text: event.target.value })}
                aria-label="Filter by name, thread, cwd, or route"
              />
            </label>
            <div className="filter-grid">
              <label>Kind<select value={agentFilter.kind} onChange={(event) => setAgentFilter({ ...agentFilter, kind: event.target.value })}>
                <option value="all">All kinds</option>
                {agentCounts.kinds.map(([kind, count]) => <option key={kind} value={kind}>{kind} ({count})</option>)}
              </select></label>
              <label>Route<select value={agentFilter.route} onChange={(event) => setAgentFilter({ ...agentFilter, route: event.target.value })}>
                <option value="all">All routes</option>
                {agentCounts.routes.map(([route, count]) => <option key={route} value={route}>{route} ({count})</option>)}
              </select></label>
              <label>Status<select value={agentFilter.status} onChange={(event) => setAgentFilter({ ...agentFilter, status: event.target.value })}>
                <option value="all">All statuses</option>
                {agentCounts.statuses.map(([status, count]) => <option key={status} value={status}>{status} ({count})</option>)}
              </select></label>
              <label>Chat<select value={agentFilter.chatStatus} onChange={(event) => setAgentFilter({ ...agentFilter, chatStatus: event.target.value })}>
                <option value="all">All chats</option>
                {agentCounts.chatStatuses.map(([status, count]) => <option key={status} value={status}>{status} ({count})</option>)}
              </select></label>
            </div>
            <p className="agent-count">{filteredAgents.length} of {safeAgents.length} agents shown</p>
          </div>
          <div className="agent-list">
            {safeAgents.length === 0 ? <Empty text="No agents returned by daemon." /> : filteredAgents.map((agent) => (
              <article key={agent.name} className="agent-row">
                <div>
                  <strong>{agent.name}</strong>
                  <span>{agent.kind} / {routeLabel(agent.route)} / runtime {agent.status} / chat {chatStatusLabel(agent.chat_status)} via {chatStatusSourceLabel(agent.chat_status_source)}</span>
                </div>
                <code>{agent.thread_id || 'no thread'}</code>
                <div className="row-actions">
                  <button
                    type="button"
                    className="compact"
                    onClick={() => setSendForm({ ...sendForm, target_agent: agent.name })}
                  >
                    <Send size={16} />Use target
                  </button>
                  <button type="button" className="compact" onClick={() => readAgent(agent.name)} disabled={busy}>
                    <Eye size={16} />Read
                  </button>
                  <button type="button" className="compact" onClick={() => resumeAgent(agent.name)} disabled={busy}>
                    <RotateCw size={16} />Resume
                  </button>
                </div>
              </article>
            ))}
            {safeAgents.length > 0 && filteredAgents.length === 0 && <Empty text="No agents match the current filters." />}
          </div>
        </div>

        <form className="panel" onSubmit={sendMessage}>
          <header><MessageSquareText size={18} /><h2>Send Message</h2></header>
          <label>Target agent<input value={sendForm.target_agent} onChange={(e) => setSendForm({ ...sendForm, target_agent: e.target.value })} required /></label>
          <label>From agent<input value={sendForm.source_agent} onChange={(e) => setSendForm({ ...sendForm, source_agent: e.target.value })} /></label>
          <label>Correlation ID<input value={sendForm.correlation_id} onChange={(e) => setSendForm({ ...sendForm, correlation_id: e.target.value })} /></label>
          <label>Message type<input value={sendForm.message_type} onChange={(e) => setSendForm({ ...sendForm, message_type: e.target.value })} /></label>
          <label className="checkbox">
            <input type="checkbox" checked={sendForm.strict} onChange={(e) => setSendForm({ ...sendForm, strict: e.target.checked })} />
            Strict delivery: fail loudly instead of silently queueing when conversational delivery is impossible
          </label>
          <label>Message<textarea value={sendForm.body} onChange={(e) => setSendForm({ ...sendForm, body: e.target.value })} required /></label>
          <button className="primary" disabled={busy}><Send size={18} />Send</button>
          {lastSendResult && (
            <div className={`send-result ${lastSendResult.ok ? 'good' : 'bad'}`}>
              <strong>{lastSendResult.delivery}</strong>
              <span>{lastSendResult.message || lastSendResult.error || 'CAM returned delivery proof.'}</span>
              <code>{lastSendResult.message_id}</code>
              {(lastSendResult.thread_id || lastSendResult.turn_id) && (
                <small>
                  thread {lastSendResult.thread_id || 'unknown'} / turn {lastSendResult.turn_id || 'unknown'}
                </small>
              )}
            </div>
          )}
        </form>

        <form className="panel" onSubmit={createAgent}>
          <header><ListRestart size={18} /><h2>Create Agent</h2></header>
          <label>Name<input value={agentForm.name} onChange={(e) => setAgentForm({ ...agentForm, name: e.target.value })} required /></label>
          <label>Source<select value={agentForm.source} onChange={(e) => setAgentForm({ ...agentForm, source: e.target.value })}><option value="codex">Codex</option><option value="agy">Antigravity</option><option value="mailbox">Mailbox</option></select></label>
          <label>Working directory<input value={agentForm.cwd} onChange={(e) => setAgentForm({ ...agentForm, cwd: e.target.value })} /></label>
          <label>Thread/session ID<input value={agentForm.thread_id} onChange={(e) => setAgentForm({ ...agentForm, thread_id: e.target.value })} /></label>
          <button className="primary" disabled={busy}><Users size={18} />Create</button>
        </form>

        <div className="panel feed">
          <header><Inbox size={18} /><h2>Inbox</h2></header>
          {inbox?.messages?.length ? inbox.messages.map((message) => (
            <article key={message.message_id} className="message">
              <strong>{message.source_agent || 'unknown'} to {message.target_agent}</strong>
              <p>{message.body}</p>
            </article>
          )) : <Empty text="No inbox messages." />}
        </div>
      </section>

      {operationResult && (
        <section className="panel operation-panel">
          <header><Eye size={18} /><h2>{operationResult.label}</h2></header>
          <OperationSummary value={operationResult.result} />
          <pre>{JSON.stringify(compactOperationResult(operationResult.result), null, 2)}</pre>
        </section>
      )}

      <section className="panel log-panel">
        <header><Activity size={18} /><h2>Logs</h2></header>
        <pre>{JSON.stringify(logs, null, 2)}</pre>
      </section>
    </main>
  );
}

function Metric({ icon, label, value, detail }) {
  return <article className="metric">{React.cloneElement(icon, { size: 22 })}<span>{label}</span><strong>{value}</strong><small>{detail}</small></article>;
}

function asArray(value) {
  return Array.isArray(value) ? value : [];
}

function Empty({ text }) {
  return <p className="empty">{text}</p>;
}

function textOf(value) {
  return typeof value === 'string' ? value : JSON.stringify(value);
}

function sendNotice(result) {
  if (!result) return 'CAM returned no delivery result';
  const flags = [
    result.delivered ? 'delivered' : null,
    result.received ? 'received' : null,
    result.queued ? 'queued' : null
  ].filter(Boolean);
  return flags.length ? `Message ${flags.join(', ')}` : `Message ${result.delivery || 'processed'}`;
}

function operationNotice(label, result) {
  if (label === 'Local discovery') {
    return `Discovery scanned ${result.rows_discovered ?? 0} rows and promoted ${result.promoted ?? 0}`;
  }
  if (label === 'Peer sync') {
    return `Peer sync requested ${result.peers_requested ?? 0} peers`;
  }
  if (label.startsWith('Resume ')) {
    return `Resume result: ${result.status || result.state || result.event_type || 'returned'}`;
  }
  if (label.startsWith('Read ')) {
    return `Read ${result.mailbox_message_count ?? 0} mailbox messages; transcript ${result.provider_transcript_status || 'unknown'}`;
  }
  return `${label} finished`;
}

function OperationSummary({ value }) {
  if (!value || typeof value !== 'object') return <p className="empty">{textOf(value)}</p>;
  const rows = summaryRows(value);
  if (rows.length === 0) return <p className="empty">Operation returned no summary fields.</p>;
  return (
    <dl className="summary-grid">
      {rows.map(([label, detail]) => (
        <React.Fragment key={label}>
          <dt>{label}</dt>
          <dd>{detail}</dd>
        </React.Fragment>
      ))}
    </dl>
  );
}

function summaryRows(value) {
  if ('rows_discovered' in value || 'promoted' in value) {
    return [
      ['Rows discovered', value.rows_discovered ?? 0],
      ['Approved', value.approved ?? 0],
      ['Candidates', value.candidate ?? 0],
      ['Quarantined', value.quarantined ?? 0],
      ['Rejected', value.rejected ?? 0],
      ['Promoted', value.promoted ?? 0]
    ];
  }
  if ('peers_requested' in value || 'results' in value) {
    return [
      ['Peers requested', value.peers_requested ?? 0],
      ['Succeeded', value.peers_synced ?? value.succeeded ?? value.synced ?? 0],
      ['Failed', value.peers_failed ?? value.failed ?? 0],
      ['Results', Array.isArray(value.results) ? value.results.length : 0]
    ];
  }
  if ('mailbox_message_count' in value || 'provider_transcript_status' in value) {
    return [
      ['Agent', value.agent?.name || 'unknown'],
      ['Mailbox messages', value.mailbox_message_count ?? 0],
      ['Evidence scope', value.evidence_scope || 'unknown'],
      ['Transcript source', value.transcript_source || 'unknown'],
      ['Provider transcript', value.provider_transcript_status || 'unknown']
    ];
  }
  if ('agent' in value || 'status' in value) {
    return [
      ['Agent', value.agent || value.name || 'unknown'],
      ['Status', value.status || value.state || 'unknown'],
      ['Message', value.message || value.error || 'none']
    ];
  }
  return Object.entries(value)
    .slice(0, 6)
    .map(([key, detail]) => [key, typeof detail === 'object' ? JSON.stringify(detail) : String(detail)]);
}

function peerHealth(peers) {
  if (!peers.length) return 'no enrolled peers';
  const failed = peers.filter((peer) => String(peer.state || '').includes('failed')).length;
  return failed ? `${failed} peers need attention` : 'peer inventory loaded';
}

function summarizeAgents(agents) {
  return {
    kinds: countBy(agents, (agent) => agent.kind || 'unknown'),
    routes: countBy(agents, (agent) => routeLabel(agent.route)),
    statuses: countBy(agents, (agent) => agent.status || 'unknown'),
    chatStatuses: countBy(agents, (agent) => chatStatusLabel(agent.chat_status))
  };
}

function countBy(items, keyOf) {
  const counts = new Map();
  for (const item of items) {
    const key = keyOf(item);
    counts.set(key, (counts.get(key) || 0) + 1);
  }
  return [...counts.entries()].sort((left, right) => left[0].localeCompare(right[0]));
}

function agentMatchesFilter(agent, filter) {
  if (filter.kind !== 'all' && agent.kind !== filter.kind) return false;
  if (filter.status !== 'all' && agent.status !== filter.status) return false;
  if (filter.chatStatus !== 'all' && chatStatusLabel(agent.chat_status) !== filter.chatStatus) return false;
  const route = routeLabel(agent.route);
  if (filter.route !== 'all' && route !== filter.route) return false;
  const needle = filter.text.trim().toLowerCase();
  if (!needle) return true;
  return [
    agent.name,
    agent.thread_id,
    agent.cwd,
    agent.kind,
    route,
    agent.status,
    agent.chat_status,
    agent.chat_status_source,
    agent.model,
    agent.model_provider
  ].some((value) => String(value || '').toLowerCase().includes(needle));
}

function compactOperationResult(value) {
  if (!value || typeof value !== 'object') return value;
  const result = { ...value };
  for (const key of ['promotion_decisions', 'results', 'mailbox_messages']) {
    if (Array.isArray(result[key]) && result[key].length > 25) {
      result[`${key}_total`] = result[key].length;
      result[key] = result[key].slice(0, 25);
      result[`${key}_ui_note`] = 'bounded to first 25 rows in the desktop preview';
    }
  }
  return result;
}

function routeLabel(route) {
  if (!route) return 'unknown route';
  if (typeof route === 'string') return route;
  if (route.peer_name) return `peer:${route.peer_name}`;
  if (route.peer?.peer_name) return `peer:${route.peer.peer_name}`;
  if (route.Peer?.peer_name) return `peer:${route.Peer.peer_name}`;
  if (route.Local !== undefined) return 'local';
  return JSON.stringify(route);
}

function chatStatusLabel(value) {
  return value || 'unknown';
}

function chatStatusSourceLabel(value) {
  return value || 'unknown';
}

createRoot(document.getElementById('root')).render(<App />);
