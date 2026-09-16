<script lang="ts">
  import type { ComponentProps } from 'svelte';
  import SingleEncode from '$lib/components/shared/MappedSourceEncode.svelte';

  let props: Omit<ComponentProps<typeof SingleEncode>, 'backend'> = $props();
</script>

<SingleEncode {...props} backend="standalone" />
