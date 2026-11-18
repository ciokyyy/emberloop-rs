<script lang="ts">
  import { getIcon } from 'material-file-icons';
  import { fileName } from '$lib/fileEmbeds';
  let { path }: { path: string } = $props();
  // Same extension icon a file embed shows; an unknown extension falls back to the library's document.
  const icon = $derived(getIcon(fileName(path)));
</script>

<span class="md-file-icon" aria-hidden="true" data-file-icon={icon.name}>{@html icon.svg}</span>
