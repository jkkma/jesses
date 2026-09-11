<script lang="ts">
  import type { ComponentProps } from 'svelte';
  import SingleEncode from '$lib/components/shared/SingleEncode.svelte';

  let props: Omit<ComponentProps<typeof SingleEncode>, 'backend'> = $props();
</script>

<SingleEncode {...props} backend="av1an" />
