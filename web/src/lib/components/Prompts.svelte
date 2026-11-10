<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import type { PromptInfo, PromptProposal } from '$lib/api/types';

  let prompts = $state<PromptInfo[]>([]);
  let proposals = $state<PromptProposal[]>([]);
  let selected = $state<string | null>(null);
  let text = $state('');
  let error = $state('');
  let saving = $state(false);

  onMount(() => void load());

  async function load() {
    try {
      [prompts, proposals] = await Promise.all([api.listPrompts(), api.listPromptProposals()]);
    } catch (err) {
      error = String(err);
    }
  }

  async function edit(id: string) {
    selected = id;
    error = '';
    try {
      text = (await api.getPrompt(id)).text;
    } catch (err) {
      error = String(err);
    }
  }

  async function save() {
    if (!selected) return;
    saving = true;
    error = '';
    try {
      await api.updatePrompt(selected, text);
      await load();
    } catch (err) {
      error = String(err);
    } finally {
      saving = false;
    }
  }

  async function reset() {
    if (!selected) return;
    saving = true;
    error = '';
    try {
      await api.deletePrompt(selected);
      await load();
      text = (await api.getPrompt(selected)).text;
    } catch (err) {
      error = String(err);
    } finally {
      saving = false;
    }
  }

  async function decide(id: string, approve: boolean) {
    saving = true;
    error = '';
    try {
      if (approve) await api.approvePromptProposal(id);
      else await api.discardPromptProposal(id);
      await load();
      if (selected === id) text = (await api.getPrompt(id)).text;
    } catch (err) {
      error = String(err);
    } finally {
      saving = false;
    }
  }
</script>

<section class="flex flex-col gap-3">
  <h3 class="text-[1.0625rem] font-semibold">Prompts</h3>
  <p class="text-[13px] text-fg-3">
    Edits apply to new Sessions only. A running Session keeps the text it pinned.
  </p>
  {#if error}<p class="text-[13px] text-status-needs">{error}</p>{/if}
  {#if proposals.length}
    <div class="flex flex-col gap-2 rounded-2xl border border-border bg-surface-1 p-3">
      <h4 class="text-[13px] font-semibold">Proposed changes ({proposals.length})</h4>
      <p class="text-[13px] text-fg-3">
        Staged by the background review. Nothing applies until you approve it.
      </p>
      {#each proposals as proposal (proposal.id)}
        <div class="flex flex-col gap-2 rounded-xl border border-border p-2">
          <div class="flex items-center justify-between gap-2">
            <span class="font-mono text-[13px]">{proposal.id}</span>
            {#if proposal.stale}<span class="chip">stale</span>{/if}
          </div>
          <p class="text-[13px]">{proposal.reason}</p>
          <div class="grid gap-2 sm:grid-cols-2">
            <div>
              <p class="text-[12px] text-fg-3">Current</p>
              <pre class="max-h-[240px] overflow-auto whitespace-pre-wrap font-mono text-[12px]">{proposal.current}</pre>
            </div>
            <div>
              <p class="text-[12px] text-fg-3">Proposed</p>
              <pre class="max-h-[240px] overflow-auto whitespace-pre-wrap font-mono text-[12px]">{proposal.text}</pre>
            </div>
          </div>
          <div class="flex justify-end gap-2">
            <button class="btn" disabled={saving} onclick={() => void decide(proposal.id, false)}
              >Reject</button
            >
            <button
              class="btn btn-pri"
              disabled={saving}
              onclick={() => void decide(proposal.id, true)}>Approve</button
            >
          </div>
        </div>
      {/each}
    </div>
  {/if}
  <div class="grid gap-3 sm:grid-cols-[220px_1fr]">
    <div class="flex flex-col gap-1 rounded-2xl border border-border bg-surface-1 p-2">
      {#each prompts as prompt (prompt.id)}
        <button
          class="flex items-center justify-between gap-2 rounded-lg px-2 py-1.5 text-left font-mono text-[13px] hover:bg-surface-2"
          class:text-accent={selected === prompt.id}
          onclick={() => void edit(prompt.id)}
        >
          <span>{prompt.id}</span>
          {#if prompt.differs}<span class="chip">edited</span>{/if}
        </button>
      {/each}
    </div>
    <div class="flex flex-col gap-2 rounded-2xl border border-border bg-surface-1 p-3">
      {#if selected}
        <div class="flex items-center justify-between gap-2">
          <span class="font-mono text-[13px]">{selected}</span>
          <div class="flex gap-2">
            <button class="btn" disabled={saving} onclick={() => void reset()}>Reset</button>
            <button class="btn btn-pri" disabled={saving} onclick={() => void save()}>Save</button>
          </div>
        </div>
        <textarea class="field min-h-[320px] font-mono text-[12px]" bind:value={text} spellcheck="false"
        ></textarea>
      {:else}
        <p class="text-[13px] text-fg-3">Choose a prompt to edit it.</p>
      {/if}
    </div>
  </div>
</section>
