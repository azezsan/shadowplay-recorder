<!--
  One row of the setup checklist: whether the check holds and, when it does
  not, what the user must do. `action` is where a check's fix button goes.
-->
<script lang="ts">
  import CircleAlert from "@lucide/svelte/icons/circle-alert";
  import CircleCheck from "@lucide/svelte/icons/circle-check";
  import type { Snippet } from "svelte";

  let {
    title,
    ready,
    help,
    steps = [],
    action,
  }: {
    /** What the check shows, phrased for its current state. */
    title: string;
    ready: boolean;
    /** What to do, shown only when the check does not hold. */
    help?: string;
    /** Numbered steps after `help`, for instructions with several clicks. */
    steps?: string[];
    action?: Snippet;
  } = $props();
</script>

<li class="flex gap-3">
  {#if ready}
    <CircleCheck class="mt-0.5 size-5 shrink-0 text-emerald-600" aria-label="Ready" />
  {:else}
    <CircleAlert class="mt-0.5 size-5 shrink-0 text-amber-600" aria-label="Needs attention" />
  {/if}
  <div class="flex min-w-0 flex-1 flex-col gap-1.5">
    <p class="font-medium">{title}</p>
    {#if !ready}
      {#if help}
        <p class="text-muted-foreground text-sm">{help}</p>
      {/if}
      {#if steps.length > 0}
        <ol class="text-muted-foreground list-decimal space-y-0.5 pl-5 text-sm">
          {#each steps as step (step)}
            <li>{step}</li>
          {/each}
        </ol>
      {/if}
      {#if action}
        <div class="pt-1">{@render action()}</div>
      {/if}
    {/if}
  </div>
</li>
