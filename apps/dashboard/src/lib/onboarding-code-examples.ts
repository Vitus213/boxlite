import definitions from './quickstart/interfaces.json'
import cTemplate from './quickstart/templates/c.c?raw'
import cliTemplate from './quickstart/templates/cli.sh?raw'
import previewServer from './quickstart/templates/preview-server.py?raw'
import goTemplate from './quickstart/templates/go.go?raw'
import pythonTemplate from './quickstart/templates/python.py?raw'
import restTemplate from './quickstart/templates/rest.sh?raw'
import restWaitTemplate from './quickstart/templates/rest-wait.sh?raw'
import rustTemplate from './quickstart/templates/rust.rs?raw'
import typescriptTemplate from './quickstart/templates/typescript.mts?raw'
import type {
  OnboardingCodeExample,
  QuickstartInterfaceDefinition,
  RenderOnboardingCodeExampleOptions,
} from './quickstart/types'

export type OnboardingInterface = string
export type { OnboardingCodeExample, QuickstartInterfaceDefinition }

type TemplateValues = Record<string, string>

const templateMap: Record<string, string> = {
  c: cTemplate,
  cli: cliTemplate,
  go: goTemplate,
  python: pythonTemplate,
  rest: restTemplate,
  rust: rustTemplate,
  typescript: typescriptTemplate,
}

const quickstartDefinitions = definitions as QuickstartInterfaceDefinition[]

function doubleQuoted(value: string): string {
  return `"${value.replace(/\\/g, '\\\\').replace(/"/g, '\\"').replace(/\n/g, '\\n')}"`
}

function shellSingleQuoted(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`
}

function shellApiKeyBlock(apiKey?: string): string {
  if (apiKey) {
    return `BOXLITE_API_KEY=${shellSingleQuoted(apiKey)}
export BOXLITE_API_KEY`
  }

  return `if [ -z "\${BOXLITE_API_KEY:-}" ]; then
  printf "Paste your BoxLite API key from Step 1: " >&2
  if [ -t 0 ]; then stty -echo; fi
  IFS= read -r BOXLITE_API_KEY
  if [ -t 0 ]; then stty echo; fi
  printf "\\n" >&2
fi
export BOXLITE_API_KEY`
}

function buildTemplateValues(options: RenderOnboardingCodeExampleOptions): TemplateValues {
  const apiKey = options.apiKey
  const command = options.command ?? { executable: 'echo', args: ['Hello from BoxLite SDK'] }
  return {
    REST_EXECUTION_RESULT: restWaitTemplate.trimEnd(),
    SERVER_PY: `"""${previewServer.trimEnd()}"""`,
    SERVER_TS: `\`${previewServer.trimEnd()}\``,
    SERVER_GO: `\`${previewServer.trimEnd()}\``,
    SERVER_RS: `r#"${previewServer.trimEnd()}"#`,
    SERVER_C: previewServer
      .trimEnd()
      .split('\n')
      .map((line) => doubleQuoted(line + '\n'))
      .join('\n        '),
    SERVER_SH: shellSingleQuoted(previewServer.trimEnd()),
    COMMAND_LITERAL: doubleQuoted(command.executable),
    ARGUMENT_LITERALS: command.args.map(doubleQuoted).join(', '),
    ARGUMENT_COUNT: String(command.args.length),
    RUST_ARGUMENTS: command.args.map((arg) => `.arg(${doubleQuoted(arg)})`).join(''),
    C_ARGUMENTS: (options.command?.args ?? ['Hello from BoxLite C SDK']).map(doubleQuoted).join(', '),
    SHELL_COMMAND: options.command
      ? [command.executable, ...command.args].map(shellSingleQuoted).join(' ')
      : 'echo "Hello from BoxLite CLI"',
    EXEC_JSON_SH: shellSingleQuoted(
      JSON.stringify({ command: command.executable, args: options.command?.args ?? ['Hello from BoxLite REST'] }),
    ),
    API_KEY_C: apiKey ? doubleQuoted(apiKey) : 'getenv("BOXLITE_API_KEY")',
    API_KEY_GO: apiKey ? doubleQuoted(apiKey) : 'os.Getenv("BOXLITE_API_KEY")',
    API_KEY_PY: apiKey ? doubleQuoted(apiKey) : 'os.environ["BOXLITE_API_KEY"]',
    API_KEY_RS: apiKey ? `${doubleQuoted(apiKey)}.to_owned()` : 'std::env::var("BOXLITE_API_KEY")?',
    API_KEY_SH: shellApiKeyBlock(apiKey),
    API_KEY_TS: apiKey ? doubleQuoted(apiKey) : 'process.env.BOXLITE_API_KEY',
    REST_API_URL: options.restApiUrl,
  }
}

function renderTemplate(template: string, values: TemplateValues): string {
  const rendered = template.replace(/\{\{([A-Z0-9_]+)\}\}/g, (match, key: string) => {
    const value = values[key]
    if (value === undefined) {
      throw new Error(`Missing quickstart template value: ${key}`)
    }
    return value
  })

  const unresolved = rendered.match(/\{\{[^}]+}}/)
  if (unresolved) {
    throw new Error(`Unresolved quickstart template token: ${unresolved[0]}`)
  }

  return rendered.trimEnd()
}

function renderCommand(command: string, values: TemplateValues): string {
  return renderTemplate(command, values)
}

function getDefinition(id: OnboardingInterface): QuickstartInterfaceDefinition {
  const definition = quickstartDefinitions.find((item) => item.id === id)
  if (!definition) {
    throw new Error(`Unknown quickstart interface: ${id}`)
  }
  return definition
}

function renderSections(definition: QuickstartInterfaceDefinition, values: TemplateValues): string[] {
  const template = templateMap[definition.template]
  if (!template) throw new Error(`Missing quickstart template: ${definition.template}`)
  const sections = template.split(/\{\{STEP_(?:EXECUTION|PREVIEW)\}\}/)
  if (sections.length !== 3) throw new Error(`Missing quickstart sections for ${definition.id}`)
  return sections.map((part) => renderTemplate(part, values).replace(/^\n+/, ''))
}

function renderExample(definition: QuickstartInterfaceDefinition, values: TemplateValues): OnboardingCodeExample {
  return {
    ...definition,
    install: renderCommand(definition.install, values),
    run: renderCommand(definition.run, values),
    example: renderSections(definition, values).join('\n\n'),
  }
}

export function getOnboardingInterfaces(): QuickstartInterfaceDefinition[] {
  return quickstartDefinitions
}

export function getOnboardingCodeExamples(): Record<OnboardingInterface, OnboardingCodeExample> {
  const values = buildTemplateValues({ restApiUrl: 'your-api-url' })
  return Object.fromEntries(
    quickstartDefinitions.map((definition) => [definition.id, renderExample(definition, values)]),
  )
}

export function renderOnboardingCodeExample(
  selectedInterface: OnboardingInterface,
  options: RenderOnboardingCodeExampleOptions,
): string {
  return renderExample(getDefinition(selectedInterface), buildTemplateValues(options)).example
}

// These sections share program scope and must be saved in order in one file.
export function renderOnboardingCodeSteps(
  selectedInterface: OnboardingInterface,
  options: RenderOnboardingCodeExampleOptions,
) {
  const [creation, execution, preview] = renderSections(getDefinition(selectedInterface), buildTemplateValues(options))
  return { creation, execution, preview, example: [creation, execution, preview].join('\n\n') }
}
