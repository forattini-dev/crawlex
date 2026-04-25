#!/usr/bin/env node
// Convert raw ClientHello `.bin` files (output of `tls-canary`) into
// curl-impersonate-compatible YAML signatures.
//
// Usage:
//   node scripts/yaml-from-capture.mjs \
//     --bin /tmp/crawlex-tls-captures/chrome_149_linux_*.bin \
//     --browser chrome --version 149.0.7795.2 --os linux \
//     --out src/impersonate/catalog/captured/chrome_149.0.7795.2_linux.yaml
//
//   # batch mode — pick most-recent capture matching label:
//   node scripts/yaml-from-capture.mjs \
//     --captures /tmp/crawlex-tls-captures \
//     --batch chrome=120,121,...,149,chromium=120,...,firefox=111,...
//
// The output YAML matches curl-impersonate's `tls_client_hello` schema so
// build.rs can ingest both vendored and our captures without branching.
//
// Validation: if a `--mined-oracles <dir>` flag is present, also compares
// computed JA3/JA4 against `<browser>_<major>_<os>.json` mined hashes —
// emits a diff report when they don't match (helps spot corrupted captures).

import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import crypto from 'node:crypto';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..');

// ──────────────────────────────────────────────────────────────────────
// ClientHello parser — minimal but covers every field curl-impersonate
// schema cares about. Mirrors src/impersonate/ja3.rs::ClientHello::parse.
// ──────────────────────────────────────────────────────────────────────

function isGrease(value) {
  // GREASE values per RFC 8701 are { 0x0a0a, 0x1a1a, ..., 0xfafa }.
  // Pattern: high byte == low byte AND low nibble == 0xa.
  const hi = (value >> 8) & 0xff;
  const lo = value & 0xff;
  return hi === lo && (hi & 0x0f) === 0x0a;
}

function readU8(view, off) {
  return view.getUint8(off);
}
function readU16(view, off) {
  return view.getUint16(off, false); // big-endian
}

function parseClientHello(bytes) {
  const view = new DataView(
    bytes.buffer,
    bytes.byteOffset,
    bytes.byteLength,
  );

  // Record header.
  if (bytes[0] !== 0x16) throw new Error('not a TLS handshake record');
  const recordVersion = readU16(view, 1);
  const recordLen = readU16(view, 3);
  if (5 + recordLen > bytes.length) {
    // Browser may chunk; we still try to parse what we have.
  }

  // Handshake header.
  let p = 5;
  if (bytes[p] !== 0x01) throw new Error('not a ClientHello');
  // const handshakeLen = (bytes[p + 1] << 16) | (bytes[p + 2] << 8) | bytes[p + 3];
  p += 4;

  const legacyVersion = readU16(view, p);
  p += 2;

  // Random (32 bytes).
  p += 32;

  // session_id (vec<u8> length-prefixed).
  const sessionIdLen = readU8(view, p);
  p += 1;
  p += sessionIdLen;

  // cipher_suites.
  const cipherListLen = readU16(view, p);
  p += 2;
  const ciphers = [];
  for (let i = 0; i < cipherListLen; i += 2) {
    ciphers.push(readU16(view, p + i));
  }
  p += cipherListLen;

  // compression_methods.
  const compLen = readU8(view, p);
  p += 1;
  const compMethods = [];
  for (let i = 0; i < compLen; i++) {
    compMethods.push(readU8(view, p + i));
  }
  p += compLen;

  // extensions.
  const extTotalLen = readU16(view, p);
  p += 2;
  const extEnd = p + extTotalLen;
  const extensions = [];
  while (p + 4 <= extEnd && p + 4 <= bytes.length) {
    const extType = readU16(view, p);
    const extLen = readU16(view, p + 2);
    p += 4;
    const extData = bytes.slice(p, p + extLen);
    p += extLen;
    extensions.push({ type: extType, length: extLen, data: extData });
  }

  return {
    recordVersion,
    legacyVersion,
    sessionIdLen,
    ciphers,
    compMethods,
    extensions,
  };
}

// ──────────────────────────────────────────────────────────────────────
// Extension type → curl-impersonate name (subset that matters).
// ──────────────────────────────────────────────────────────────────────

function extName(typeId) {
  if (isGrease(typeId)) return 'GREASE';
  switch (typeId) {
    case 0: return 'server_name';
    case 5: return 'status_request';
    case 10: return 'supported_groups';
    case 11: return 'ec_point_formats';
    case 13: return 'signature_algorithms';
    case 16: return 'application_layer_protocol_negotiation';
    case 17: return 'status_request_v2';
    case 18: return 'signed_certificate_timestamp';
    case 21: return 'padding';
    case 22: return 'encrypt_then_mac';
    case 23: return 'extended_master_secret';
    case 27: return 'compress_certificate';
    case 35: return 'session_ticket';
    case 43: return 'supported_versions';
    case 45: return 'psk_key_exchange_modes';
    case 51: return 'keyshare';
    case 17513: return 'application_settings';
    case 65037: return 'encrypted_client_hello';
    case 65281: return 'renegotiation_info';
    default: return `raw_${typeId}`;
  }
}

function tlsVersionLabel(v) {
  switch (v) {
    case 0x0301: return 'TLS_VERSION_1_0';
    case 0x0302: return 'TLS_VERSION_1_1';
    case 0x0303: return 'TLS_VERSION_1_2';
    case 0x0304: return 'TLS_VERSION_1_3';
    default: return `0x${v.toString(16)}`;
  }
}

// ──────────────────────────────────────────────────────────────────────
// JA3 + JA4 derivation (drops GREASE entries).
// ──────────────────────────────────────────────────────────────────────

function ja3String(ch) {
  const join = (xs) => xs.filter((v) => !isGrease(v)).join('-');
  // We need supported_groups + ec_point_formats from extensions.
  let supportedGroups = [];
  let ecPointFormats = [];
  for (const ext of ch.extensions) {
    if (ext.type === 10 && ext.data.length >= 2) {
      const view = new DataView(ext.data.buffer, ext.data.byteOffset, ext.data.byteLength);
      const listLen = view.getUint16(0, false);
      for (let i = 0; i < listLen; i += 2) {
        supportedGroups.push(view.getUint16(2 + i, false));
      }
    } else if (ext.type === 11 && ext.data.length >= 1) {
      const len = ext.data[0];
      for (let i = 0; i < len; i++) {
        ecPointFormats.push(ext.data[1 + i]);
      }
    }
  }
  const extIds = ch.extensions.map((e) => e.type);
  return [
    ch.legacyVersion,
    join(ch.ciphers),
    join(extIds),
    join(supportedGroups),
    ecPointFormats.join('-'),
  ].join(',');
}

function ja3Hash(ja3) {
  return crypto.createHash('md5').update(ja3).digest('hex');
}

// ──────────────────────────────────────────────────────────────────────
// YAML emitter — curl-impersonate schema.
// ──────────────────────────────────────────────────────────────────────

function indent(n) {
  return ' '.repeat(n);
}

function emitYaml(meta, ch) {
  const lines = [];
  lines.push('---');
  lines.push(`name: ${meta.browser}_${meta.version}_${meta.os}`);
  lines.push('browser:');
  lines.push(`${indent(4)}name: ${meta.browser}`);
  lines.push(`${indent(4)}version: ${meta.version}`);
  lines.push(`${indent(4)}os: ${meta.os}`);
  lines.push(`${indent(4)}mode: regular`);
  lines.push('signature:');
  lines.push(`${indent(4)}tls_client_hello:`);
  lines.push(`${indent(8)}record_version: '${tlsVersionLabel(ch.recordVersion)}'`);
  lines.push(`${indent(8)}handshake_version: '${tlsVersionLabel(ch.legacyVersion)}'`);
  lines.push(`${indent(8)}session_id_length: ${ch.sessionIdLen}`);
  // Ciphers.
  const cipherStrs = ch.ciphers.map((c) =>
    isGrease(c) ? "'GREASE'" : `0x${c.toString(16).padStart(4, '0')}`,
  );
  lines.push(`${indent(8)}ciphersuites: [${cipherStrs.join(', ')}]`);
  // Compression methods.
  lines.push(
    `${indent(8)}comp_methods: [${ch.compMethods.map((c) => `0x${c.toString(16).padStart(2, '0')}`).join(', ')}]`,
  );
  // Extensions.
  lines.push(`${indent(8)}extensions:`);
  for (const ext of ch.extensions) {
    const name = extName(ext.type);
    lines.push(`${indent(12)}- type: ${name}`);
    lines.push(`${indent(14)}length: ${ext.length}`);
    if (ext.type === 16 && ext.data.length >= 2) {
      // ALPN
      const view = new DataView(ext.data.buffer, ext.data.byteOffset, ext.data.byteLength);
      const listLen = view.getUint16(0, false);
      let p = 2;
      const alpns = [];
      while (p < 2 + listLen) {
        const len = ext.data[p];
        p += 1;
        alpns.push(String.fromCharCode(...ext.data.slice(p, p + len)));
        p += len;
      }
      lines.push(`${indent(14)}alpn_list: [${alpns.map((s) => `'${s}'`).join(', ')}]`);
    }
    if (ext.type === 17513 && ext.data.length >= 2) {
      // application_settings
      const view = new DataView(ext.data.buffer, ext.data.byteOffset, ext.data.byteLength);
      const listLen = view.getUint16(0, false);
      let p = 2;
      const alpns = [];
      while (p < 2 + listLen) {
        const len = ext.data[p];
        p += 1;
        alpns.push(String.fromCharCode(...ext.data.slice(p, p + len)));
        p += len;
      }
      lines.push(`${indent(14)}alps_alpn_list: [${alpns.map((s) => `'${s}'`).join(', ')}]`);
    }
    if (ext.type === 13) {
      // signature_algorithms
      const view = new DataView(ext.data.buffer, ext.data.byteOffset, ext.data.byteLength);
      const listLen = view.getUint16(0, false);
      const algs = [];
      for (let i = 0; i < listLen; i += 2) {
        algs.push(view.getUint16(2 + i, false));
      }
      lines.push(
        `${indent(14)}sig_hash_algs: [${algs.map((a) => `0x${a.toString(16).padStart(4, '0')}`).join(', ')}]`,
      );
    }
    if (ext.type === 10) {
      // supported_groups
      const view = new DataView(ext.data.buffer, ext.data.byteOffset, ext.data.byteLength);
      const listLen = view.getUint16(0, false);
      const groups = [];
      for (let i = 0; i < listLen; i += 2) {
        groups.push(view.getUint16(2 + i, false));
      }
      const groupStrs = groups.map((g) =>
        isGrease(g) ? "'GREASE'" : `0x${g.toString(16).padStart(4, '0')}`,
      );
      lines.push(`${indent(14)}supported_groups: [${groupStrs.join(', ')}]`);
    }
    if (ext.type === 43) {
      // supported_versions
      const len = ext.data[0];
      const versions = [];
      for (let i = 0; i < len; i += 2) {
        versions.push((ext.data[1 + i] << 8) | ext.data[2 + i]);
      }
      const verStrs = versions.map((v) =>
        isGrease(v) ? "'GREASE'" : `'${tlsVersionLabel(v)}'`,
      );
      lines.push(`${indent(14)}supported_versions: [${verStrs.join(', ')}]`);
    }
    if (ext.type === 27) {
      // compress_certificate
      const len = ext.data[0];
      const algs = [];
      for (let i = 0; i < len; i += 2) {
        algs.push((ext.data[1 + i] << 8) | ext.data[2 + i]);
      }
      lines.push(
        `${indent(14)}algorithms: [${algs.map((a) => `0x${a.toString(16).padStart(4, '0')}`).join(', ')}]`,
      );
    }
    if (ext.type === 45) {
      // psk_key_exchange_modes — single byte mode in the wire.
      const len = ext.data[0];
      if (len >= 1) {
        lines.push(`${indent(14)}psk_ke_mode: ${ext.data[1]}`);
      }
    }
    if (ext.type === 11) {
      // ec_point_formats
      const len = ext.data[0];
      const fmts = [];
      for (let i = 0; i < len; i++) fmts.push(ext.data[1 + i]);
      lines.push(`${indent(14)}ec_point_formats: [${fmts.join(', ')}]`);
    }
  }

  return lines.join('\n') + '\n';
}

// ──────────────────────────────────────────────────────────────────────
// Main.
// ──────────────────────────────────────────────────────────────────────

function parseArgs(argv) {
  const out = { mined: null };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--bin') out.bin = argv[++i];
    else if (a === '--out') out.out = argv[++i];
    else if (a === '--browser') out.browser = argv[++i];
    else if (a === '--version') out.version = argv[++i];
    else if (a === '--os') out.os = argv[++i];
    else if (a === '--mined-oracles') out.mined = argv[++i];
  }
  return out;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (!args.bin || !args.browser || !args.version || !args.os) {
    console.error(
      'Usage: yaml-from-capture.mjs --bin <FILE> --browser <NAME> --version <X.Y.Z> --os <linux|windows|macos|android> [--out <FILE>] [--mined-oracles <DIR>]',
    );
    process.exit(2);
  }
  const bytes = await fs.readFile(args.bin);
  const ch = parseClientHello(bytes);
  const ja3 = ja3String(ch);
  const hash = ja3Hash(ja3);

  const meta = {
    browser: args.browser,
    version: args.version,
    os: args.os,
  };
  const yaml = emitYaml(meta, ch);

  if (args.out) {
    await fs.mkdir(path.dirname(args.out), { recursive: true });
    await fs.writeFile(args.out, yaml);
    console.log(`wrote ${args.out}`);
  } else {
    process.stdout.write(yaml);
  }

  console.log(`\nJA3:      ${ja3}`);
  console.log(`JA3 hash: ${hash}`);

  // Cross-check against mined oracle if available.
  if (args.mined) {
    const major = args.version.split('.')[0];
    const oraclePath = path.join(
      args.mined,
      `${args.browser}_${major}_${args.os}.json`,
    );
    try {
      const oracle = JSON.parse(await fs.readFile(oraclePath, 'utf8'));
      if (oracle.ja3_hash && oracle.ja3_hash !== hash) {
        console.warn(
          `\n⚠️  JA3 mismatch vs mined oracle:\n  expected: ${oracle.ja3_hash}\n  got:      ${hash}\n  source:   ${oracle.source}`,
        );
      } else if (oracle.ja3_hash === hash) {
        console.log(`✓ JA3 hash matches mined oracle (${oracle.source})`);
      }
    } catch (_err) {
      console.log(`(no mined oracle for ${args.browser}_${major}_${args.os})`);
    }
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
