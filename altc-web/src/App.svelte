<script lang="ts">
  import { api, type ServerInfo } from './lib/api'
  import { session } from './lib/session'
  import Setup from './lib/Setup.svelte'
  import SignIn from './lib/SignIn.svelte'

  let info = $state<ServerInfo | null>(null)
  let failed = $state('')
  let signedIn = $state(!!session.token())

  // Which screen you get is the server's answer, not a guess: an unclaimed
  // server shows the wizard, a claimed one shows sign-in.
  $effect(() => {
    api
      .server()
      .then((i) => (info = i))
      .catch(() => (failed = 'Could not reach the server.'))
  })

  async function refresh() {
    signedIn = true
    info = await api.server().catch(() => info)
  }
</script>

<main>
  {#if failed}
    <p class="error">{failed}</p>
  {:else if !info}
    <p class="lead">Connecting…</p>
  {:else if signedIn}
    <h1>{info.name}</h1>
    <p class="lead">
      Signed in. The management UI lands here next — users, library, sharing and
      settings.
    </p>
    <button onclick={() => { session.clear(); signedIn = false }}>Sign out</button>
  {:else if info.setup_complete}
    <SignIn serverName={info.name} onDone={refresh} />
  {:else}
    <Setup serverName={info.name} onDone={refresh} />
  {/if}
</main>
