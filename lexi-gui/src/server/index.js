const http = require('http');
const { spawn } = require('child_process');
const fs = require('fs');
const path = require('path');
let YAML;
try {
  YAML = require('yaml');
} catch (err) {
  YAML = null; // will surface a helpful error later
}

const PORT = process.env.PORT || 3000;
const ROOT_DIR = path.resolve(__dirname, '..', '..'); // lexi-gui root
const WORKSPACE_ROOT = path.resolve(__dirname, '..', '..', '..'); // repo root containing cli
const PUBLIC_DIR = path.join(ROOT_DIR, 'public');
const INDEX_PATH = path.join(PUBLIC_DIR, 'index.html');
const USER_PROVIDERS_CONFIG = path.join(process.env.HOME || '', '.lexi-cli', 'providers', 'providers-config.yaml');
const REPO_PROVIDERS_CONFIG = path.join(WORKSPACE_ROOT, 'providers', 'providers-config.yaml');

const localLexiBin = path.join(WORKSPACE_ROOT, 'lexi-cli', 'cli');
const LEXI_BIN = process.env.LEXI_BIN || (fs.existsSync(localLexiBin) ? localLexiBin : 'lexi');
const LEXI_PROMPT = process.env.LEXI_PROMPT || 'lexi>';

function sendJson(res, statusCode, payload) {
  res.writeHead(statusCode, {
    'Content-Type': 'application/json',
    'Access-Control-Allow-Origin': '*',
    'X-Lexi-Prompt': LEXI_PROMPT,
  });
  res.end(JSON.stringify(payload));
}

function runLexi(args = []) {
  return new Promise((resolve) => {
    const proc = spawn(LEXI_BIN, args, {
      cwd: WORKSPACE_ROOT,
      env: { ...process.env, HOME: process.env.HOME },
    });
    let stdout = '';
    let stderr = '';
    proc.stdout.on('data', (chunk) => {
      stdout += chunk.toString();
    });
    proc.stderr.on('data', (chunk) => {
      stderr += chunk.toString();
    });
    proc.on('close', (code) => {
      if (code !== 0) {
        console.error(`[lexi-gui] lexi failed (${code}):`, LEXI_BIN, args.join(' '), stderr.trim());
      }
      resolve({ code, stdout, stderr, args });
    });
    proc.on('error', (err) => {
      console.error(`[lexi-gui] failed to spawn lexi:`, err);
      resolve({ code: 1, stdout, stderr: err.message });
    });
  });
}

function serveIndex(res) {
  const stream = fs.createReadStream(INDEX_PATH);
  let headersSent = false;
  stream.on('open', () => {
    if (!headersSent) {
      res.writeHead(200, { 'Content-Type': 'text/html' });
      headersSent = true;
    }
    stream.pipe(res);
  });
  stream.on('error', () => {
    if (!headersSent) {
      res.writeHead(500, { 'Content-Type': 'text/plain' });
      headersSent = true;
      res.end('Failed to load UI file.');
    } else {
      res.destroy();
    }
  });
}

function parsePromptList(rawOutput) {
  const text = rawOutput.trim();
  if (!text) return [];

  const attemptArrayParse = (t) => {
    try {
      const arr = JSON.parse(t);
      return Array.isArray(arr) ? arr : null;
    } catch {
      return null;
    }
  };

  // Try to coerce concatenated objects into an array
  const arrayText = `[${text.replace(/}\s*{\s*/g, '},{')}]`;
  const arr = attemptArrayParse(arrayText);
  const parts = arr && arr.length ? arr : text.split(/\n{2,}/);

  const prompts = [];
  for (const part of parts) {
    const chunk = typeof part === 'string' ? part : JSON.stringify(part);
    if (!chunk.trim()) continue;
    try {
      const obj = typeof part === 'string' ? JSON.parse(chunk) : part;
      const [name, payload] = Object.entries(obj)[0] || [];
      if (name && payload && typeof payload === 'object') {
        prompts.push({ name, ...payload });
      }
    } catch {
      continue;
    }
  }
  return prompts;
}

async function handleListPrompts(res, logFn) {
  const { code, stdout, stderr, args } = await runLexi(['prompts', 'list', '-r']);
  logFn && logFn(args);
  if (code !== 0) {
    sendJson(res, 500, { error: 'Failed to list prompts', details: stderr.trim() });
    return;
  }
  const prompts = parsePromptList(stdout);
  const normalized = prompts.map((p) => ({
    name: p.name,
    prompt: p.prompt || '',
    max_tokens: p.max_tokens,
    temperature: p.temperature,
    role: p.role,
    isActive: p.name === '$$',
  }));
  sendJson(res, 200, { prompts: normalized });
}

async function handleSavePrompt(req, res, body, logFn) {
  const { name, prompt, max_tokens, temperature, role } = body || {};
  if (!prompt || typeof prompt !== 'string') {
    sendJson(res, 400, { error: 'Prompt text is required' });
    return;
  }

  const args = ['prompts', 'set'];
  if (name) {
    args.push(name);
  }
  args.push('--prompt', prompt);
  if (max_tokens != null) {
    args.push('--max_tokens', String(max_tokens));
  }
  if (temperature != null) {
    args.push('--temperature', String(temperature));
  }
  if (role) {
    args.push('--role', role);
  }

  const { code, stderr, args: usedArgs } = await runLexi(args);
  logFn && logFn(usedArgs);
  if (code !== 0) {
    sendJson(res, 500, { error: 'Failed to save prompt', details: stderr.trim() });
    return;
  }

  await handleListPrompts(res, logFn);
}

async function handleDeletePrompt(res, name, logFn) {
  if (!name) {
    sendJson(res, 400, { error: 'Prompt name is required' });
    return;
  }
  const { code, stderr, args } = await runLexi(['prompts', 'rm', name]);
  logFn && logFn(args);
  if (code !== 0) {
    sendJson(res, 500, { error: 'Failed to delete prompt', details: stderr.trim() });
    return;
  }
  await handleListPrompts(res, logFn);
}

async function handleSetActive(res, name, logFn) {
  if (!name) {
    sendJson(res, 400, { error: 'Prompt name is required' });
    return;
  }

  // Fetch the prompt in raw form
  const listArgs = ['prompt', 'list', name, '-r'];
  const listArgs = ['prompts', 'list', name, '-r'];
  const { code: listCode, stdout, stderr: listErr, args: usedListArgs } = await runLexi(listArgs);
  logFn && logFn(usedListArgs);
  if (listCode !== 0) {
    sendJson(res, 500, { error: 'Failed to load prompt', details: listErr.trim() });
    return;
  }
  const prompts = parsePromptList(stdout);
  const payload = prompts.find((p) => p.name === name);
  if (!payload) {
    sendJson(res, 404, { error: `Prompt ${name} not found` });
    return;
  }

  const args = ['prompts', 'set', '--prompt', payload.prompt || ''];
  if (payload.max_tokens != null) args.push('--max_tokens', String(payload.max_tokens));
  if (payload.temperature != null) args.push('--temperature', String(payload.temperature));
  if (payload.role) args.push('--role', payload.role);

  const { code, stderr, args: usedSetArgs } = await runLexi(args); // no name -> active $$
  logFn && logFn(usedSetArgs);
  if (code !== 0) {
    sendJson(res, 500, { error: 'Failed to set active prompt', details: stderr.trim() });
    return;
  }

  await handleListPrompts(res, logFn);
}

function loadProvidersConfig() {
  const pathCandidates = [USER_PROVIDERS_CONFIG, REPO_PROVIDERS_CONFIG];
  const found = pathCandidates.find((p) => p && fs.existsSync(p));
  if (!found) {
    return { error: 'providers-config.yaml not found. Run lexi-cli to seed defaults or create ~/.lexi-cli/providers/providers-config.yaml.' };
  }
  if (!YAML) {
    return { error: "Missing dependency 'yaml'. Install with: npm install yaml" };
  }
  try {
    const text = fs.readFileSync(found, 'utf8');
    const data = YAML.parse(text) || {};
    return { config: data, source: found };
  } catch (err) {
    return { error: `Failed to read providers-config.yaml: ${err.message}` };
  }
}

function handleProviders(res) {
  const { config, error, source } = loadProvidersConfig();
  if (error) {
    sendJson(res, 500, { error });
    return;
  }
  const providers = (config && config.providers && typeof config.providers === 'object') ? config.providers : {};
  const normalized = Object.entries(providers).map(([name, meta]) => ({
    name,
    url: meta?.url || '',
    hosted_models: meta?.hosted_models || '',
    hosted_model_fields: Array.isArray(meta?.hosted_model_fields) ? meta.hosted_model_fields : [],
    default_model: meta?.default_model || '',
    api_key: meta?.api_key ? '[set]' : '[missing]',
    source,
  }));
  sendJson(res, 200, { providers: normalized, source });
}

function handlePrompt(req, res, logFn) {
  let body = '';
  req.on('data', (chunk) => {
    body += chunk.toString();
    if (body.length > 1e6) req.destroy();
  });

  req.on('end', () => {
    let parsed = {};
    try {
      parsed = JSON.parse(body || '{}');
    } catch (err) {
      sendJson(res, 400, { error: 'Invalid JSON body' });
      return;
    }

    if (req.method === 'GET') {
      handleListPrompts(res, logFn);
      return;
    }

    if (req.method === 'POST' && req.url === '/api/prompts') {
      handleSavePrompt(req, res, parsed, logFn);
      return;
    }

    if (req.method === 'POST' && req.url === '/api/prompts/active') {
      handleSetActive(res, parsed.name, logFn);
      return;
    }

    if (req.method === 'POST' && req.url === '/api/prompts/delete') {
      handleDeletePrompt(res, parsed.name, logFn);
      return;
    }

    sendJson(res, 404, { error: 'Not Found' });
  });
}

const server = http.createServer((req, res) => {
  if (req.method === 'GET' && (req.url === '/' || req.url === '/index.html')) {
    serveIndex(res);
    return;
  }

  if (req.url.startsWith('/api/prompts')) {
    handlePrompt(req, res, (args) => {
      res.setHeader('X-Lexi-Command', args.join(' '));
    });
    return;
  }

  if (req.method === 'GET' && (req.url === '/api/providers' || req.url === '/api/providers/')) {
    handleProviders(res);
    return;
  }

  if (req.method === 'OPTIONS') {
    res.writeHead(204, {
      'Access-Control-Allow-Origin': '*',
      'Access-Control-Allow-Methods': 'POST, GET, OPTIONS',
      'Access-Control-Allow-Headers': 'Content-Type',
    });
    res.end();
    return;
  }

  res.writeHead(404, { 'Content-Type': 'text/plain' });
  res.end('Not Found');
});

server.listen(PORT, () => {
  console.log(`GUI available at http://localhost:${PORT}`);
});
