import http from 'node:http';
import Database from 'better-sqlite3';
import Handlebars from 'handlebars';

const db = new Database('../../local/fortunes/fortunes.sqlite');
const stmt = db.prepare('SELECT id, message FROM fortune');

const template = Handlebars.compile(
  `<!DOCTYPE html><html><head><title>Fortunes</title></head><body><table><tr><th>id</th><th>message</th></tr>` +
  `{{#each fortunes}}<tr><td>{{id}}</td><td>{{message}}</td></tr>{{/each}}` +
  `</table></body></html>`
);

const server = http.createServer((req, res) => {
  if (req.url === '/fortunes') {
    const fortunes = stmt.all();
    fortunes.push({ id: 0, message: 'Additional fortune added at request time.' });
    fortunes.sort((a, b) => a.message.localeCompare(b.message));

    const html = template({ fortunes });

    res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
    res.end(html);
  } else {
    res.writeHead(404);
    res.end('Not Found');
  }
});

server.listen(8080, () => {
  console.log('listening on 8080');
});
