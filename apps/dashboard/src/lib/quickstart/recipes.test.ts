/* Copyright 2026 BoxLite AI · SPDX-License-Identifier: AGPL-3.0 */

import { spawnSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { getOnboardingInterfaces, renderOnboardingCodeExample } from '../onboarding-code-examples'
import { QUICKSTART_RECIPES, quickstartEntryFor, quickstartRecipeFor } from './recipes'

describe('Quickstart recipes', () => {
  it('waits for a REST recipe before publishing a preview and leaves its Box running', () => {
    const directory = mkdtempSync(join(tmpdir(), 'boxlite-rest-recipe-'))
    const script = renderOnboardingCodeExample('rest', {
      apiKey: 'mock',
      restApiUrl: 'https://api.test',
      command: quickstartRecipeFor('long-running-agent').command,
    })
    const fakeApi = `curl() {
      case "$*" in
        *DELETE*) printf 'delete\n' >> trace ;;
        */executions/*) if [ -f polled ]; then printf '{"status":"completed","exit_code":0}'; printf 'completed\n' >> trace; else touch polled; printf '{"status":"running","exit_code":null}'; printf 'running\n' >> trace; fi ;;
        */network/tunnel*) printf '{"uri":"https://preview.test"}'; printf 'preview\n' >> trace ;;
        */exec*) printf '{"execution_id":"exec1"}' ;;
        */start*) printf '{}' ;;
        *) printf '{"box_id":"box1"}' ;;
      esac
    }
    sleep() { :; }
    `
    try {
      const result = spawnSync('bash', ['-c', fakeApi + script], {
        cwd: directory,
        input: '\n',
        encoding: 'utf8',
        timeout: 5000,
      })
      expect(result.status, result.stderr).toBe(0)
      expect(readFileSync(join(directory, 'trace'), 'utf8').trim().split('\n')).toEqual([
        'running',
        'completed',
        'preview',
      ])
    } finally {
      rmSync(directory, { recursive: true, force: true })
    }
  })
  it('resolves examples without dropping any original task', () => {
    for (const id of [
      'coding-agent',
      'code-execution',
      'browser-agent',
      'long-running-agent',
      'application-deployments',
      'reinforcement-learning',
    ]) {
      expect(quickstartRecipeFor(id).id).toBe(id)
    }
    expect(quickstartRecipeFor('exploring').id).toBe('code-execution')
    expect(quickstartRecipeFor('unknown').id).toBe('code-execution')
    expect(QUICKSTART_RECIPES.find((recipe) => recipe.id === 'application-deployments')?.command).toBeUndefined()
  })

  it('shares a simple first task across use cases and only recommends publishing for app deployment', () => {
    for (const useCase of [
      'coding-agent',
      'code-execution',
      'browser-agent',
      'long-running-agent',
      'reinforcement-learning',
      'exploring',
      'unknown',
      undefined,
    ]) {
      expect(quickstartEntryFor(useCase).id).toBe('code-execution')
      expect(quickstartEntryFor(useCase).command?.executable).toBe('echo')
    }
    expect(quickstartEntryFor('application-deployments').id).toBe('application-deployments')
  })

  it('renders every execution recipe through all seven original interfaces', () => {
    const interfaces = getOnboardingInterfaces()
    expect(interfaces.map((item) => item.id)).toEqual(['python', 'typescript', 'go', 'rust', 'c', 'cli', 'rest'])
    for (const recipe of QUICKSTART_RECIPES.filter((item) => item.command)) {
      for (const item of interfaces) {
        const code = renderOnboardingCodeExample(item.id, { restApiUrl: 'https://api.test', command: recipe.command })
        expect(code).not.toContain('{{')
        expect(code).toContain(recipe.command!.executable)
        if (item.id === 'cli' || item.id === 'rest') {
          const result = spawnSync('bash', ['-n'], { input: code, encoding: 'utf8', timeout: 5000 })
          expect(result.stderr).toBe('')
          expect(result.status).toBe(0)
        }
        if (item.id === 'python') {
          const result = spawnSync('python3', ['-c', 'import ast,sys; ast.parse(sys.stdin.read())'], {
            input: code,
            encoding: 'utf8',
            timeout: 5000,
          })
          expect(result.stderr).toBe('')
          expect(result.status).toBe(0)
        }
      }
    }
  })

  it('ships syntactically valid Python tasks, including the browser fixture', () => {
    for (const recipe of QUICKSTART_RECIPES.filter(
      (item) => item.command?.executable === 'python3' || item.id === 'browser-agent',
    )) {
      const script =
        recipe.id === 'browser-agent'
          ? recipe.command!.args[1].split("<<'PY'\n")[1].split('\nPY')[0]
          : recipe.command!.args[1]
      const result = spawnSync('python3', ['-c', 'import ast,sys; ast.parse(sys.stdin.read())'], {
        input: script,
        encoding: 'utf8',
        timeout: 5000,
      })
      expect(result.stderr, recipe.id).toBe('')
      expect(result.status, recipe.id).toBe(0)
    }
  })

  it('runs the self-contained tasks and produces their promised results without a Box or network', () => {
    const output = (id: string) => {
      const command = quickstartRecipeFor(id).command!
      const result = spawnSync(command.executable, command.args, { encoding: 'utf8', timeout: 5000 })
      expect(result.status, `${id}: ${result.stderr}`).toBe(0)
      return result.stdout
    }
    expect(output('coding-agent')).toContain('After fix: PASS')
    expect(JSON.parse(output('long-running-agent')).total).toBe(50)
    const rollout = JSON.parse(output('reinforcement-learning'))
    expect(rollout.steps).toBe(200)
    expect(rollout.counts.reduce((sum: number, count: number) => sum + count, 0)).toBe(200)
    expect(rollout.best_arm).toBe(2)
    expect(JSON.parse(output('ci-evals')).passed).toBe(3)
    const recovery = output('state-recovery')
    expect(recovery).toContain('Resumed at 3')
    expect(recovery).toContain('Checkpoint saved: 6')
    const artifact = output('artifacts').trim().split('\n')
    expect(JSON.parse(artifact[0]).files).toEqual(['report.md', 'results.csv'])
    expect(Buffer.from(artifact[1].replace('ZIP_BASE64=', ''), 'base64').subarray(0, 4).toString('hex')).toBe(
      '504b0304',
    )
  })
})
