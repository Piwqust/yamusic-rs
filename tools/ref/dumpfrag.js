const fs = require('fs');
const src = fs.readFileSync(process.argv[2], 'utf8');
const a = src.indexOf('lt=t=>t?"vec4":"vec3",ut=(t,e)=>`');
const b = src.indexOf(',ct="', a);
const { ut } = new Function(`const ${src.slice(a, b)}; return {lt, ut};`)();
fs.writeFileSync(process.argv[3], ut(3, true));
