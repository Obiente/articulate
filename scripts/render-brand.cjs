// Developer-only export. Install @resvg/resvg-js locally before running.
// The editable mark and gradients remain SVG; ICO is the Windows shell export.
const fs = require('node:fs');
const path = require('node:path');
const { Resvg } = require(process.env.ARTICULATE_RESVG || '@resvg/resvg-js');
const root = path.resolve(__dirname, '..');
const brand = path.join(root, 'assets', 'brand');
const mark = fs.readFileSync(path.join(brand, 'articulate-mark.svg'), 'utf8').match(/<path[^>]+\/>/)[0];
const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256"><defs><radialGradient id="g"><stop stop-color="#315e53"/><stop offset="1" stop-color="#101719"/></radialGradient></defs><rect x="4" y="4" width="248" height="248" rx="56" fill="url(#g)"/><g transform="translate(33 66) scale(2.65)">${mark}</g></svg>`;
fs.writeFileSync(path.join(brand, 'app-icon.svg'), svg);
const images = [16, 24, 32, 48, 64, 128, 256].map(size => ({size, png: new Resvg(svg, {fitTo:{mode:'width',value:size}}).render().asPng()}));
const head = Buffer.alloc(6 + images.length * 16);
head.writeUInt16LE(1, 2); head.writeUInt16LE(images.length, 4);
let offset = head.length;
images.forEach(({size, png}, i) => { const at=6+i*16; head[at]=head[at+1]=size===256?0:size; head.writeUInt16LE(1,at+4);head.writeUInt16LE(32,at+6);head.writeUInt32LE(png.length,at+8);head.writeUInt32LE(offset,at+12);offset+=png.length; });
fs.writeFileSync(path.join(brand, 'articulate.ico'),Buffer.concat([head,...images.map(image=>image.png)]));
fs.writeFileSync(path.join(brand, 'app-icon.png'), images.at(-1).png);
