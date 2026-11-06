<script lang="ts">
  import { Switch } from 'bits-ui';

  let {
    checked = $bindable(false),
    disabled = false,
    label,
    onchange
  }: {
    checked?: boolean;
    disabled?: boolean;
    label: string;
    onchange?: (checked: boolean) => void;
  } = $props();
</script>

<Switch.Root bind:checked {disabled} aria-label={label} class="switch" onCheckedChange={onchange}>
  <Switch.Thumb class="switch-thumb" />
</Switch.Root>
