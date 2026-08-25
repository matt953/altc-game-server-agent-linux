<script lang="ts">
  import { api, ApiError } from './api'
  import { session } from './session'

  let { serverName, onDone }: { serverName: string; onDone: () => void } = $props()

  let name = $state('')
  let password = $state('')
  let busy = $state(false)
  let error = $state('')

  async function signIn(event: Event) {
    event.preventDefault()
    if (busy) return
    busy = true
    error = ''
    try {
      const { token } = await api.login(name.trim(), password)
      session.save(token)
      onDone()
    } catch (e) {
      error = e instanceof ApiError ? e.message : 'Could not reach the server'
      busy = false
    }
  }
</script>

<form onsubmit={signIn}>
  <h1>{serverName}</h1>

  <label>
    Name
    <input bind:value={name} autocomplete="username" autofocus />
  </label>

  <label>
    Password
    <input type="password" bind:value={password} autocomplete="current-password" />
  </label>

  {#if error}
    <p class="error" role="alert">{error}</p>
  {/if}

  <button type="submit" disabled={busy}>{busy ? 'Signing in…' : 'Sign in'}</button>
</form>
