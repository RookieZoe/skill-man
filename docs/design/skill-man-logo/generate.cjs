/* global require, __dirname, Buffer, process */
/* eslint @typescript-eslint/no-require-imports: "off" -- Standalone CommonJS Node asset generator. */
const fs = require('node:fs');
const path = require('node:path');
const sharp = require(process.env.SHARP_MODULE || 'sharp');
const out = __dirname;
const blue = '#2474DB';
const symbol = (color, width = 3) => `<g fill="none" stroke="${color}" stroke-width="${width}" stroke-linecap="round" stroke-linejoin="round"><path d="M17 5H8.5a3.5 3.5 0 0 0 0 7h1"/><path d="M7 19h8.5a3.5 3.5 0 0 0 0-7h-1"/></g>`;
const svg = (size, content) => `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 ${size} ${size}">${content}</svg>`;
const app = svg(1024, `<rect x="64" y="64" width="896" height="896" rx="200" fill="${blue}"/><g transform="translate(176 176) scale(28)">${symbol('#FFFFFF',3.2)}</g>`);
const mark = svg(24, symbol('#17191D'));
// Optical master: full-pixel 2px strokes and a 2px gap between the modules at 1x.
const tray = svg(18, `<g fill="none" stroke="#000" stroke-width="2" stroke-linecap="round"><path d="M13 4H6.5a2.5 2.5 0 0 0 0 5H7"/><path d="M5 14h6.5a2.5 2.5 0 0 0 0-5H11"/></g>`);
(async () => {
  for (const [name, source] of [['app-icon.svg',app],['mark.svg',mark],['menubar-template.svg',tray]]) fs.writeFileSync(path.join(out,name),source+'\n');
  for (const size of [16,32,64,128,256,512,1024]) await sharp(Buffer.from(app)).resize(size,size).png().toFile(path.join(out,`app-icon-${size}.png`));
  for (const size of [18,36]) await sharp(Buffer.from(tray)).resize(size,size).png().toFile(path.join(out,`menubar-template-${size}.png`));
  fs.writeFileSync(path.join(out,'menubar-template-18.rgba'),await sharp(Buffer.from(tray)).ensureAlpha().raw().toBuffer());
  const embed = (source,x,y,w,h=w) => `<image x="${x}" y="${y}" width="${w}" height="${h}" href="data:image/svg+xml;base64,${Buffer.from(source).toString('base64')}"/>`;
  const text = (x,y,t,size=14,color='#70757F',weight=400) => `<text x="${x}" y="${y}" font-family="Helvetica,Arial,sans-serif" font-size="${size}" font-weight="${weight}" fill="${color}">${t}</text>`;
  const whiteTray=tray.replaceAll('stroke="#000"','stroke="#FFF"');
  const board=`<svg xmlns="http://www.w3.org/2000/svg" width="1440" height="1020" viewBox="0 0 1440 1020">
  <rect width="1440" height="1020" fill="#F5F5F3"/>
  ${text(72,73,'SKILL MAN',17,'#17191D',600)}${text(72,114,'Two modules. One S.',30,'#17191D',500)}${text(1130,73,'IDENTITY / 01',13)}
  <line x1="72" y1="151" x2="1368" y2="151" stroke="#DADCD9"/>
  ${text(72,193,'01   APP ICON',12)}${embed(app,110,215,430)}
  ${text(665,193,'02   THE MARK',12)}${embed(mark,810,265,240)}
  ${text(720,565,'A shared shape for skills, connection and order.',17)}
  <line x1="72" y1="696" x2="1368" y2="696" stroke="#DADCD9"/>
  ${text(72,737,'03   AT ACTUAL SIZE',12)}
  ${embed(app,74,767,128)}${embed(app,239,799,64)}${embed(app,344,823,32)}${embed(app,418,835,16)}
  ${text(109,923,'128',12)}${text(262,923,'64',12)}${text(350,923,'32',12)}${text(419,923,'16',12)}
  ${text(665,737,'04   MENU BAR / 18 PT',12)}
  <rect x="665" y="781" width="703" height="46" rx="9" fill="#E4E5E7"/>
  ${text(684,810,'Skill Man',13,'#21242A',600)}${text(1128,810,'100%',12,'#21242A')}${embed(tray,1090,795,18)}${text(1190,810,'Fri  10:09',13,'#21242A')}
  <rect x="665" y="843" width="703" height="46" rx="9" fill="#25282E"/>
  ${text(684,872,'Skill Man',13,'#F3F4F6',600)}${text(1128,872,'100%',12,'#F3F4F6')}${embed(whiteTray,1090,857,18)}${text(1190,872,'Fri  10:09',13,'#F3F4F6')}
  ${text(665,932,'Single-color template. Optically adjusted for small sizes.',13)}
  </svg>`;
  fs.writeFileSync(path.join(out,'preview.svg'),board);
  await sharp(Buffer.from(board)).png().toFile(path.join(out,'preview.png'));
})();
