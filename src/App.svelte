<script lang="ts">
  import ActivityPanel from "$lib/components/ActivityPanel.svelte";
  import SettingsPanel from "$lib/components/SettingsPanel.svelte";
  import SetupChecklist from "$lib/components/SetupChecklist.svelte";
  import StatusPanel from "$lib/components/StatusPanel.svelte";
  import { followStatus } from "$lib/status.svelte";

  void followStatus();
</script>

<main class="mx-auto flex max-w-xl flex-col gap-4 p-4">
  <StatusPanel />
  <SetupChecklist />
  <SettingsPanel />
  <ActivityPanel />
</main>
