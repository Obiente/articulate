import fs from 'node:fs';
import crypto from 'node:crypto';
import path from 'node:path';
const [privatePath,publicPath] = process.argv.slice(2);
let directory = fs.realpathSync(path.dirname(path.resolve(privatePath)));
for (;;) {
    if (fs.existsSync(path.join(directory,'.git'))) throw Error('Private key must be outside a repository');
    const parent = path.dirname(directory);
    if (parent === directory) break;
    directory = parent;
}
const {privateKey,publicKey} = crypto.generateKeyPairSync('ed25519');
fs.writeFileSync(privatePath,privateKey.export({format:'pem',type:'pkcs8'}),{flag:'wx',mode:0o600});
fs.writeFileSync(publicPath,publicKey.export({format:'der',type:'spki'}).subarray(-32).toString('hex')+'\n');
