import { existsSync } from 'node:fs'
import { join } from 'node:path'

type Mode = 'analyze' | 'implement'

interface Options {
  mode: Mode
  model: string
  effort: 'low' | 'medium' | 'high' | 'xhigh' | 'max'
  resume?: string
  task: string
}

const READ_ONLY_COMMANDS = [
  'PowerShell(git status *)',
  'PowerShell(git diff *)',
  'PowerShell(git log *)',
  'PowerShell(git show *)',
  'PowerShell(git rev-parse *)',
  'PowerShell(git ls-files *)',
  'PowerShell(rg *)',
]

const VERIFICATION_COMMANDS = [
  'PowerShell(bun run format)',
  'PowerShell(bun run format:check)',
  'PowerShell(bun run lint:ui *)',
  'PowerShell(bun run test:ui *)',
  'PowerShell(bun run --filter ui lint *)',
  'PowerShell(bun run --filter ui test *)',
  'PowerShell(bun cargo fmt)',
  'PowerShell(bun cargo fmt *)',
  'PowerShell(bun cargo check *)',
  'PowerShell(bun cargo test *)',
  'PowerShell(cargo fmt)',
  'PowerShell(cargo fmt *)',
  'PowerShell(cargo check *)',
  'PowerShell(cargo test *)',
]

const DENIED_TOOLS = [
  'Agent',
  'WebFetch',
  'WebSearch',
  'Bash',
  'PowerShell(git add *)',
  'PowerShell(git commit *)',
  'PowerShell(git push *)',
  'PowerShell(git reset *)',
  'PowerShell(git checkout *)',
  'PowerShell(git clean *)',
  'PowerShell(git restore *)',
  'PowerShell(winget *)',
  'PowerShell(Invoke-RestMethod *)',
  'PowerShell(Invoke-WebRequest *)',
  'PowerShell(curl.exe *)',
  'PowerShell(Start-Process *)',
  'PowerShell(Remove-Item *)',
  'Read(.env)',
  'Read(**/.env*)',
]

const WORKER_CONTRACT = `You are a subordinate workhorse in a Codex-orchestrated workflow.
Codex owns scope, integration, verification, and user communication. Read the repository-root AGENTS.md and CLAUDE.md completely before doing substantive work.

Stay inside the exact delegated task. Treat every pre-existing working-tree change as user-owned: never revert, overwrite, reformat, or otherwise disturb unrelated edits. Do not commit, stage, push, reset, checkout, clean, restore, delete files, install software, fetch from the web, change machine configuration, or modify agent configuration. Do not read secrets such as .env files or credential stores.

For implementation work, make the smallest coherent change that meets the stated acceptance criteria and run only relevant verification. If the task is ambiguous, conflicts with existing edits, or needs authority outside the task, stop and explain the blocker instead of guessing.

End with a compact handoff containing: Outcome, Files changed, Verification, and Risks or follow-ups. Be exact about commands actually run and failures encountered.`

function usage(exitCode = 2): never {
  console.error(`Usage:
  bun run claude:worker --mode analyze -- "<task>"
  bun run claude:worker --mode implement -- "<task>"

Options:
  --mode <analyze|implement>  Worker permission profile (default: analyze)
  --model <alias-or-id>       Claude model override (default: claude-opus-5)
  --effort <level>            low, medium, high, xhigh, or max
  --resume <session-id>       Continue a previous worker session
  --help                      Show this help

If no task argument is provided, the task is read from stdin.`)
  process.exit(exitCode)
}

function parseArgs(args: string[]): Options {
  let mode: Mode = 'analyze'
  let model = 'claude-opus-5'
  let effort: Options['effort'] | undefined
  let resume: string | undefined
  const taskParts: string[] = []
  let positionalOnly = false

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index]
    if (positionalOnly) {
      taskParts.push(arg)
      continue
    }
    if (arg === '--') {
      positionalOnly = true
      continue
    }
    if (arg === '--help' || arg === '-h') usage(0)

    const nextValue = () => {
      const value = args[index + 1]
      if (!value) usage()
      index += 1
      return value
    }

    if (arg === '--mode') {
      const value = nextValue()
      if (value !== 'analyze' && value !== 'implement') usage()
      mode = value
    } else if (arg === '--model') {
      model = nextValue()
    } else if (arg === '--effort') {
      const value = nextValue()
      if (!['low', 'medium', 'high', 'xhigh', 'max'].includes(value)) usage()
      effort = value as Options['effort']
    } else if (arg === '--resume') {
      resume = nextValue()
    } else if (arg.startsWith('-')) {
      usage()
    } else {
      taskParts.push(arg)
    }
  }

  return {
    mode,
    model,
    effort: effort ?? (mode === 'implement' ? 'high' : 'medium'),
    resume,
    task: taskParts.join(' ').trim(),
  }
}

function resolveClaude(): string {
  const onPath = Bun.which('claude')
  if (onPath) return onPath

  const userProfile = process.env.USERPROFILE
  if (userProfile) {
    const nativeInstall = join(userProfile, '.local', 'bin', 'claude.exe')
    if (existsSync(nativeInstall)) return nativeInstall
  }

  throw new Error(
    'Claude Code is not installed or cannot be found. Install it from https://code.claude.com/docs/en/installation and authenticate once with `claude`.',
  )
}

async function main() {
  const options = parseArgs(Bun.argv.slice(2))
  if (!options.task && !process.stdin.isTTY) {
    options.task = (await Bun.stdin.text()).trim()
  }
  if (!options.task) usage()

  const availableTools = ['Read', 'Glob', 'Grep', 'PowerShell']
  const allowedTools = ['Read', 'Glob', 'Grep', ...READ_ONLY_COMMANDS]
  if (options.mode === 'implement') {
    availableTools.push('Edit', 'Write')
    allowedTools.push('Edit', 'Write', ...VERIFICATION_COMMANDS)
  }

  const args = [
    '-p',
    '--output-format',
    'json',
    '--restricted',
    '--strict-mcp-config',
    '--no-chrome',
    '--tools',
    availableTools.join(','),
    '--permission-mode',
    'dontAsk',
    '--allowedTools',
    ...allowedTools,
    '--disallowedTools',
    ...DENIED_TOOLS,
    '--effort',
    options.effort,
    '--model',
    options.model,
    '--append-system-prompt',
    WORKER_CONTRACT,
    '--system-prompt-snapshot',
    'off',
    '--name',
    `codex-${options.mode}-worker`,
  ]

  if (options.resume) args.push('--resume', options.resume)
  args.push(options.task)

  const claude = resolveClaude()
  console.error(
    `[claude-worker] mode=${options.mode} effort=${options.effort} model=${options.model}`,
  )

  const child = Bun.spawn([claude, ...args], {
    cwd: process.cwd(),
    env: {
      ...process.env,
      CLAUDE_CODE_USE_POWERSHELL_TOOL: '1',
    },
    stdin: 'ignore',
    stdout: 'pipe',
    stderr: 'pipe',
  })

  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ])

  if (stderr.trim()) console.error(stderr.trim())

  let payload: Record<string, unknown> | undefined
  try {
    payload = JSON.parse(stdout) as Record<string, unknown>
  } catch {
    if (stdout.trim()) console.log(stdout.trim())
  }

  if (payload) {
    const result = payload.result
    if (typeof result === 'string' && result.trim()) console.log(result.trim())
    else console.log(JSON.stringify(payload, null, 2))

    const metadata = [
      typeof payload.session_id === 'string' ? `session=${payload.session_id}` : undefined,
      typeof payload.num_turns === 'number' ? `turns=${payload.num_turns}` : undefined,
      typeof payload.total_cost_usd === 'number'
        ? `estimated_cost_usd=${payload.total_cost_usd.toFixed(4)}`
        : undefined,
    ].filter(Boolean)
    if (metadata.length) console.error(`[claude-worker] ${metadata.join(' ')}`)
  }

  if (exitCode !== 0) {
    console.error(`[claude-worker] Claude exited with code ${exitCode}.`)
    process.exit(exitCode)
  }
}

await main()
