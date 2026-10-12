// Local-only site for browser tool acceptance. No production credentials or data.
import http from 'node:http';
import { readFile } from 'node:fs/promises';
import { randomBytes } from 'node:crypto';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.dirname(fileURLToPath(import.meta.url));
const token = randomBytes(24).toString('hex');
const port = Number(process.argv[2] ?? 17841);
const base = 'http://127.0.0.1:' + port;
const server = http.createServer(async (request, response) => {
  const route = new URL(request.url ?? '/', base).pathname;
  const send = (status, type, body, headers = {}) => {
    response.writeHead(status, { 'content-type': type, 'cache-control': 'no-store', ...headers });
    response.end(body);
  };
  if (route === '/download') {
    send(200, 'text/plain', 'Pixels browser acceptance fixture\n', {
      'content-disposition': 'attachment; filename="result.txt"',
    });
  } else if (route === '/second') {
    send(200, 'text/html; charset=utf-8', '<!doctype html><title>Second page</title><h1>Second page</h1><a href="/">Back to fixture</a>');
  } else if (route === '/login' && request.method === 'POST') {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const body = new URLSearchParams(Buffer.concat(chunks).toString('utf8'));
    if (body.get('username') === 'demo' && body.get('password') === 'demo') {
      response.writeHead(303, { location: '/account', 'set-cookie': `pixels_fixture=${token}; Max-Age=3600; HttpOnly; SameSite=Lax; Path=/` });
      response.end();
    } else send(401, 'text/html; charset=utf-8', '<!doctype html><title>Sign in failed</title><h1>Sign in failed</h1><a href="/account">Try again</a>');
  } else if (route === '/demo-sign-in' && request.method === 'GET') {
    response.writeHead(303, { location: '/account', 'set-cookie': `pixels_fixture=${token}; Max-Age=3600; HttpOnly; SameSite=Lax; Path=/` });
    response.end();
  } else if (route === '/logout') {
    response.writeHead(303, { location: '/account', 'set-cookie': 'pixels_fixture=; Max-Age=0; HttpOnly; SameSite=Lax; Path=/' });
    response.end();
  } else if (route === '/account') {
    const authorized = (request.headers.cookie ?? '').split(';').some(value => value.trim() === `pixels_fixture=${token}`);
    send(200, 'text/html; charset=utf-8', authorized
      ? '<!doctype html><title>Account</title><h1>Signed in as demo</h1><a href="/logout">Sign out</a>'
      : '<!doctype html><title>Sign in</title><h1>Sign in required</h1><a href="/demo-sign-in">Continue as demo (test only)</a><form method="post" action="/login"><label>Username <input name="username"></label><label>Password <input name="password" type="password"></label><button>Sign in</button></form>');
  } else if (route === '/') {
    send(200, 'text/html; charset=utf-8', await readFile(path.join(root, 'acceptance.html')));
  } else send(404, 'text/plain', 'Not found');
});
server.listen(port, '127.0.0.1', () => console.log(`READY ${base}`));
