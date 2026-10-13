import net from 'node:net';
import http from 'node:http';

const echoPort = Number(process.argv[2] ?? 38673);
const httpPort = Number(process.argv[3] ?? 38674);
if (!Number.isInteger(echoPort) || !Number.isInteger(httpPort) || echoPort < 1 || httpPort < 1) {
  throw new Error('usage: node port-forward-server.mjs ECHO_PORT HTTP_PORT');
}

const echo = net.createServer({ allowHalfOpen: true }, (socket) => {
  socket.on('data', (chunk) => {
    if (!socket.write(chunk)) socket.pause();
  });
  socket.on('drain', () => socket.resume());
  socket.on('end', () => socket.end());
  socket.on('error', () => socket.destroy());
});
const web = http.createServer((request, response) => {
  response.writeHead(200, { 'content-type': 'text/plain; charset=utf-8' });
  response.end(`pixels-port-forward:${request.url}`);
});
await Promise.all([
  new Promise((resolve, reject) => echo.listen(echoPort, '127.0.0.1', resolve).once('error', reject)),
  new Promise((resolve, reject) => web.listen(httpPort, '127.0.0.1', resolve).once('error', reject)),
]);
console.log(JSON.stringify({ echoPort, httpPort, pid: process.pid }));
