<script lang="ts">
  import { api, ApiError } from './api'
  import { session } from './session'

  let { serverName, onDone }: { serverName: string; onDone: () => void } = $props()

  let name = $state('')
  let password = $state('')
  let confirm = $state('')
  let busy = $state(false)
  let error = $state('')

  // Mirrors the server's rule rather than inventing a stricter one: a form
  // that rejects what the API accepts is its own kind of bug.
  const MIN = 8
  let problem = $derived(
    name.trim() === ''
      ? 'Choose a name'
      : password.length < MIN
        ? `Password must be at least ${MIN} characters`
        : password !== confirm
          ? 'Passwords do not match'
          : '',
  )

  async function claim(event: Event) {
    event.preventDefault()
    if (problem || busy) return
    busy = true
    error = ''
    try {
      const { token } = await api.claim(name.trim(), password)
      session.save(token)
      onDone()
    } catch (e) {
      error = e instanceof ApiError ? e.message : 'Could not reach the server'
      busy = false
    }
  }
</script>

<form onsubmit={claim}>
  <h1>Set up {serverName}</h1>
  <p class="lead">
    This creates the owner account. It happens once — after this, the server
    can't be claimed by anyone else.
  </p>

  <label>
    Your name
    <input bind:value={name} autocomplete="username" autofocus />
  </label>

  <label>
    Password
    <input type="password" bind:value={password} autocomplete="new-password" />
  </label>

  <label>
    Confirm password
    <input type="password" bind:value={confirm} autocomplete="new-password" />
  </label>

  {#if error}
    <p class="error" role="alert">{error}</p>
  {/if}

  <button type="submit" disabled={!!problem || busy}>
    {busy ? 'Setting up…' : 'Claim this server'}
  </button>

  <!-- Shown rather than blocking: telling someone what is wrong before they
       submit beats a red message after they do. -->
  {#if problem && (name || password || confirm)}
    <p class="hint">{problem}</p>
  {/if}
</form>
