import { mount } from 'svelte';
import CommandPlanPreview from '../../../src/lib/components/shared/CommandPlanPreview.svelte';
import type { EncodeRequest } from '../../../src/lib/ipc/generated';

const target = document.createElement('div');
document.body.append(target);
mount(CommandPlanPreview, { target, props: { request: {} as EncodeRequest } });
