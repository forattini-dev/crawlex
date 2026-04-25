#!/usr/bin/env node

import process from 'node:process'
import path from 'node:path'
import { pathToFileURL } from 'node:url'

function readArg(name, fallback = undefined) {
  const idx = process.argv.indexOf(name)
  if (idx === -1) return fallback
  return process.argv[idx + 1] ?? fallback
}

const raffelPath = readArg('--raffel-path')
const host = readArg('--host', '127.0.0.1')
const port = Number.parseInt(readArg('--port', '8899'), 10)

if (!raffelPath) {
  console.error('missing --raffel-path')
  process.exit(2)
}
if (!Number.isInteger(port) || port <= 0 || port > 65535) {
  console.error(`invalid --port: ${port}`)
  process.exit(2)
}

const modulePath = path.join(raffelPath, 'dist', 'index.js')
const mod = await import(pathToFileURL(modulePath).href)
const { createExplicitProxy } = mod

if (typeof createExplicitProxy !== 'function') {
  console.error(`createExplicitProxy not found in ${modulePath}`)
  process.exit(2)
}

const proxy = createExplicitProxy({
  host,
  port,
  tunnel: { mode: 'forward' },
  telemetry: {
    metricsEndpoint: '/metrics',
    graphEndpoint: '/proxy/graph',
    defaultLabels: { proxy: 'raffel-local' },
  },
})

let stopping = false

async function shutdown(code = 0) {
  if (stopping) return
  stopping = true
  try {
    await proxy.stop()
  } catch (err) {
    console.error(err)
    code = code || 1
  }
  process.exit(code)
}

process.on('SIGINT', () => void shutdown(0))
process.on('SIGTERM', () => void shutdown(0))

try {
  const boundPort = await proxy.start()
  console.log(`READY http://${host}:${boundPort}`)
} catch (err) {
  console.error(err)
  process.exit(1)
}
