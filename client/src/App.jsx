import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

const STREAMING_CMDS = ['flood'];
const JWT_PRESETS = {
  viewer: {
    label: 'Viewer',
    claims: {
      roles: ['viewer'],
      plan: 'basic',
      permissions: ['read'],
    },
  },
  operator: {
    label: 'Operator',
    claims: {
      roles: ['operator'],
      plan: 'pro',
      permissions: ['read', 'write'],
    },
  },
  admin: {
    label: 'Admin + Billing',
    claims: {
      roles: ['admin', 'billing'],
      plan: 'enterprise',
      permissions: ['read', 'write', 'billing', 'admin'],
      custom_flag: true,
    },
  },
};

function fmtUs(us) {
  if (us >= 1000000) return `${(us / 1000000).toFixed(1)}s`;
  if (us >= 1000) return `${(us / 1000).toFixed(1)}ms`;
  return `${us}us`;
}

function fmtBytes(bytes) {
  if (bytes >= 1048576) return `${(bytes / 1048576).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${bytes} B`;
}

export default function App() {
  const [lines, setLines] = useState([]);
  const [inputValue, setInputValue] = useState('');
  const [history, setHistory] = useState([]);
  const [historyIndex, setHistoryIndex] = useState(-1);
  const [inputDisabled, setInputDisabled] = useState(false);
  const [placeholder, setPlaceholder] = useState('help');
  const [jwtUsername, setJwtUsername] = useState('');
  const [jwtOrg, setJwtOrg] = useState('');
  const [jwtPreset, setJwtPreset] = useState('viewer');
  const [jwtToken, setJwtToken] = useState('');
  const [jwtError, setJwtError] = useState('');
  const [jwtLoading, setJwtLoading] = useState(false);

  const terminalRef = useRef(null);
  const inputRef = useRef(null);
  const activeStreamRef = useRef(null);
  const activeTransportRef = useRef(null);

  const addLine = useCallback((cls, text) => {
    setLines((prev) => [...prev, { cls, text }]);
  }, []);

  const stopStream = useCallback(() => {
    if (activeTransportRef.current) {
      activeTransportRef.current.close();
      activeTransportRef.current = null;
    }
    if (activeStreamRef.current) {
      activeStreamRef.current.close();
      activeStreamRef.current = null;
    }
    addLine('system', '^C');
    addLine('', '');
    setInputDisabled(false);
    setPlaceholder('help');
    inputRef.current?.focus();
  }, [addLine]);

  const endStreamUI = useCallback(() => {
    addLine('muted', '(stream ended)');
    addLine('', '');
    setInputDisabled(false);
    setPlaceholder('help');
    inputRef.current?.focus();
  }, [addLine]);

  const startSSE = useCallback(
    (cmd) => {
      const es = new EventSource(`/exec/stream?cmd=${encodeURIComponent(cmd)}`);
      activeStreamRef.current = es;

      es.onmessage = (e) => {
        addLine('stream', e.data);
      };

      es.onerror = () => {
        es.close();
        if (activeStreamRef.current === es) {
          activeStreamRef.current = null;
          endStreamUI();
        }
      };
    },
    [addLine, endStreamUI]
  );

  const startWebTransport = useCallback(async () => {
    try {
      const wtPort = location.port === '443' || location.port === '' ? 443 : 4433;
      const wtHost = wtPort === 443 ? location.hostname : '127.0.0.1';
      const wt = new WebTransport(`https://${wtHost}:${wtPort}/`);
      activeTransportRef.current = wt;
      await wt.ready;
    } catch {
      activeTransportRef.current = null;
      return false;
    }

    try {
      addLine('system', 'webtransport connected (quic datagrams)');
      const reader = activeTransportRef.current.datagrams.readable.getReader();
      const decoder = new TextDecoder();

      while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        const text = decoder.decode(value);
        text.split('\n').forEach((line) => {
          if (line.trim()) {
            addLine('stream', line);
          }
        });
      }
    } catch {
      // Stream interrupted by cancel/disconnect.
    } finally {
      activeTransportRef.current = null;
      endStreamUI();
    }

    return true;
  }, [addLine, endStreamUI]);

  const startStream = useCallback(
    async (cmd) => {
      setInputDisabled(true);
      setPlaceholder('ctrl+c to stop');

      if (typeof WebTransport !== 'undefined') {
        const ok = await startWebTransport(cmd);
        if (!ok) {
          addLine('muted', 'falling back to sse');
          startSSE(cmd);
        }
        return;
      }

      startSSE(cmd);
    },
    [addLine, startSSE, startWebTransport]
  );

  const runCommand = useCallback(
    async (cmd) => {
      addLine('prompt-color', `> ${cmd}`);
      const baseCmd = cmd.split(/\s+/)[0];

      if (STREAMING_CMDS.includes(baseCmd)) {
        await startStream(cmd);
        return;
      }

      setInputDisabled(true);

      try {
        const t0 = performance.now();
        const resp = await fetch(`/exec?cmd=${encodeURIComponent(cmd)}`);
        const data = await resp.json();
        const httpMs = (performance.now() - t0).toFixed(1);

        if (data.error) {
          addLine('error', data.error);
        } else {
          const stdout = data.stdout.trimEnd();
          const stderr = data.stderr.trimEnd();
          const metrics = data.metrics;

          if (stdout) addLine('stdout', stdout);
          if (stderr) addLine('stderr', stderr);

          const parts = [
            `compile ${fmtUs(metrics.compile_us)}`,
            `run ${fmtUs(metrics.run_us)}`,
            `total ${fmtUs(metrics.total_us)}`,
            `http ${httpMs}ms`,
            `${fmtBytes(metrics.wasm_size_bytes)} wasm`,
          ];
          addLine('metrics', parts.join('  ·  '));
        }
      } catch (err) {
        addLine('error', `request failed: ${err.message}`);
      }

      addLine('', '');
      setInputDisabled(false);
      inputRef.current?.focus();
    },
    [addLine, startStream]
  );

  const generateToken = useCallback(async () => {
    setJwtError('');
    setJwtLoading(true);

    const username = jwtUsername.trim();
    const org = jwtOrg.trim();

    if (!username || !org) {
      setJwtLoading(false);
      setJwtError('username and org are required');
      return;
    }

    const preset = JWT_PRESETS[jwtPreset] || JWT_PRESETS.viewer;

    try {
      const response = await fetch('/jwt', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          username,
          org,
          ...preset.claims,
        }),
      });

      const data = await response.json();
      if (!response.ok || data.error) {
        throw new Error(data.error || `request failed with ${response.status}`);
      }

      setJwtToken(data.token || '');
      addLine('system', `jwt issued for ${username}@${org} (${preset.label} preset)`);
      addLine('', '');
    } catch (err) {
      setJwtToken('');
      setJwtError(`token request failed: ${err.message}`);
    } finally {
      setJwtLoading(false);
    }
  }, [addLine, jwtOrg, jwtPreset, jwtUsername]);

  const onInputKeyDown = useCallback(
    async (e) => {
      if (e.key === 'c' && e.ctrlKey) {
        e.preventDefault();
        stopStream();
        return;
      }

      if (e.key === 'ArrowUp') {
        e.preventDefault();
        if (historyIndex < history.length - 1) {
          const nextIndex = historyIndex + 1;
          setHistoryIndex(nextIndex);
          setInputValue(history[history.length - 1 - nextIndex]);
        }
        return;
      }

      if (e.key === 'ArrowDown') {
        e.preventDefault();
        if (historyIndex > 0) {
          const nextIndex = historyIndex - 1;
          setHistoryIndex(nextIndex);
          setInputValue(history[history.length - 1 - nextIndex]);
        } else {
          setHistoryIndex(-1);
          setInputValue('');
        }
        return;
      }

      if (e.key !== 'Enter' || activeStreamRef.current) return;

      const cmd = inputValue.trim();
      if (!cmd) return;

      setHistory((prev) => [...prev, cmd]);
      setHistoryIndex(-1);
      setInputValue('');
      await runCommand(cmd);
    },
    [history, historyIndex, inputValue, runCommand, stopStream]
  );

  useEffect(() => {
    addLine('system', 'wasi preview 2 interactive shell');
    addLine(
      'muted',
      'commands run as wasm components with wasi:cli, wasi:clocks, wasi:random, wasi:sockets, wasi:filesystem'
    );

    if (typeof WebTransport !== 'undefined') {
      addLine('muted', 'flood streams via quic datagrams (webtransport)');
    } else {
      addLine(
        'muted',
        'flood streams via sse (launch chrome with --origin-to-force-quic-on for quic datagrams)'
      );
      fetch('/cert-hash')
        .then((r) => r.text())
        .then((hash) => {
          addLine(
            'muted',
            `  open -na "Google Chrome" --args --origin-to-force-quic-on=127.0.0.1:4433 --ignore-certificate-errors-spki-list=${hash} http://localhost:3000`
          );
        })
        .catch(() => {});
    }

    addLine('muted', 'type "help" to see available commands');
    addLine('', '');
  }, [addLine]);

  useEffect(() => {
    if (terminalRef.current) {
      terminalRef.current.scrollTo({ top: terminalRef.current.scrollHeight });
    }
  }, [lines]);

  useEffect(() => {
    const onDocKeyDown = (e) => {
      if (e.key === 'c' && e.ctrlKey && (activeStreamRef.current || activeTransportRef.current)) {
        e.preventDefault();
        stopStream();
      }
    };

    document.addEventListener('keydown', onDocKeyDown);
    return () => {
      document.removeEventListener('keydown', onDocKeyDown);
      if (activeStreamRef.current) activeStreamRef.current.close();
      if (activeTransportRef.current) activeTransportRef.current.close();
    };
  }, [stopStream]);

  const renderedLines = useMemo(
    () =>
      lines.map((line, idx) => (
        <div key={idx} className={`line ${line.cls}`}>
          {line.text}
        </div>
      )),
    [lines]
  );

  return (
    <div className="app-shell">
      <div id="header">
        <h1>v8-matrix</h1>
        <span>wasi preview 2 shell · each command runs inside a sandboxed wasm component</span>
      </div>

      <div className="jwt-panel">
        <div className="jwt-row">
          <input
            type="text"
            value={jwtUsername}
            onChange={(e) => setJwtUsername(e.target.value)}
            placeholder="username"
            autoComplete="off"
            spellCheck={false}
          />
          <input
            type="text"
            value={jwtOrg}
            onChange={(e) => setJwtOrg(e.target.value)}
            placeholder="org"
            autoComplete="off"
            spellCheck={false}
          />
          <select value={jwtPreset} onChange={(e) => setJwtPreset(e.target.value)}>
            {Object.entries(JWT_PRESETS).map(([key, preset]) => (
              <option key={key} value={key}>
                {preset.label}
              </option>
            ))}
          </select>
          <button type="button" onClick={generateToken} disabled={jwtLoading}>
            {jwtLoading ? 'Generating...' : 'Generate token'}
          </button>
        </div>
        {jwtError ? <div className="jwt-error">{jwtError}</div> : null}
        {jwtToken ? <textarea className="jwt-token" value={jwtToken} readOnly rows={3} /> : null}
      </div>

      <div id="terminal" ref={terminalRef} tabIndex={-1} onClick={() => inputRef.current?.focus()}>
        {renderedLines}
      </div>

      <div id="input-row">
        <span className="prompt">&gt;</span>
        <input
          id="input"
          ref={inputRef}
          type="text"
          value={inputValue}
          onChange={(e) => setInputValue(e.target.value)}
          onKeyDown={onInputKeyDown}
          placeholder={placeholder}
          disabled={inputDisabled}
          autoFocus
          autoComplete="off"
          spellCheck={false}
        />
      </div>
    </div>
  );
}
