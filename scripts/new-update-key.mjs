import fs from 'node:fs';
import crypto from 'node:crypto';
const [privatePath,publicPath] = process.argv.slice(2);
const {privateKey,publicKey} = crypto.generateKeyPairSync('ed25519');
fs.writeFileSync(privatePath,privateKey.export({format:'pem',type:'pkcs8'}),{flag:'wx',mode:0o600});
fs.writeFileSync(publicPath,publicKey.export({format:'der',type:'spki'}).subarray(-32).toString('hex')+'\n');
