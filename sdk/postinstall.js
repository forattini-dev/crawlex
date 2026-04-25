#!/usr/bin/env node
'use strict';

const path = require('node:path');
const { ensureInstalled } = require('./crawlex-sdk');

const SKIP = '1';
const targetDir = path.join(__dirname, '..', '.crawlex', 'bin');
const shouldSkip = process.env.CRAWLEX_SKIP_POSTINSTALL === SKIP;
const allowFailure = process.env.CRAWLEX_POSTINSTALL_ALLOW_FAILURE === SKIP;
const verify = process.env.CRAWLEX_POSTINSTALL_NO_VERIFY !== SKIP;

if (shouldSkip) {
  process.stdout.write('crawlex: postinstall skipped by CRAWLEX_SKIP_POSTINSTALL=1\n');
  process.exit(0);
}

const options = { targetDir, verify, skipIfFresh: false };
if (process.env.CRAWLEX_POSTINSTALL_CHANNEL) options.channel = process.env.CRAWLEX_POSTINSTALL_CHANNEL;
if (process.env.CRAWLEX_POSTINSTALL_VERSION) options.version = process.env.CRAWLEX_POSTINSTALL_VERSION;
if (process.env.GITHUB_TOKEN) options.githubToken = process.env.GITHUB_TOKEN;

ensureInstalled(options)
  .then((r) => {
    const how = r.changed ? 'installed' : 'already present';
    process.stdout.write(`crawlex: ${how} at ${r.binaryPath}\n`);
  })
  .catch((err) => {
    process.stderr.write(
      `crawlex: postinstall failed — ${err.message}\n` +
      `  the package will not work until a binary is present.\n` +
      `  workarounds:\n` +
      `    • rerun \`pnpm install --force crawlex\`\n` +
      `    • set CRAWLEX_FORCE_BINARY=/absolute/path/to/crawlex\n` +
      `    • set CRAWLEX_POSTINSTALL_ALLOW_FAILURE=1 to continue without a binary\n`
    );
    process.exit(allowFailure ? 0 : 1);
  });
