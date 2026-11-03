import { copyFileSync, mkdirSync } from 'node:fs';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '../..');
const output = resolve(root, 'scratch/showcase-pages');
mkdirSync(output, { recursive: true });
copyFileSync(resolve(root, 'docs/showcase.html'), resolve(output, 'index.html'));
copyFileSync(resolve(root, 'app/public/guide.html'), resolve(output, 'guide.html'));
copyFileSync(resolve(root, 'app/public/legal.html'), resolve(output, 'legal.html'));
console.log('GitHub Pages site prepared at', output);
