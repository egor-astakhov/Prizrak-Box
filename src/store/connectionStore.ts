import { defineStore } from 'pinia';
import { shallowRef } from 'vue';

export type ConnectionViewMode = 'list' | 'topology' | 'process';

const MAX_CLOSED = 500;

// Closed connections live OUTSIDE the store state. The persistence plugin
// subscribes to the state with a deep watcher, so as part of the state every
// websocket frame (once a second) walked all of them — up to 500 objects with
// their metadata — just to persist `viewMode`, a cost that grew with the list
// for as long as the Connections page was open. The list is replaced (never
// mutated in place), so a shallowRef keeps it reactive for the view.
const closed = shallowRef<any[]>([]);

export const useConnectionStore = defineStore('connections', {
  state: () => ({
    viewMode: 'topology' as ConnectionViewMode,
    showClosed: false,
  }),
  getters: {
    closedConnections: (): any[] => closed.value,
  },
  actions: {
    setViewMode(viewMode: ConnectionViewMode) {
      this.viewMode = viewMode;
    },
    toggleViewMode() {
      this.viewMode = this.viewMode === 'list' ? 'topology' : 'list';
    },
    setShowClosed(val: boolean) {
      this.showClosed = val;
    },
    recordClosed(prevSnapshot: any[], nextSnapshot: any[]) {
      const nextIds = new Set<string>(nextSnapshot.map((c: any) => c.id));
      const justClosed: any[] = [];
      const now = Date.now();
      for (const conn of prevSnapshot) {
        if (!nextIds.has(conn.id)) {
          justClosed.push({ ...conn, closedAt: now });
        }
      }
      if (justClosed.length === 0) {
        return;
      }
      closed.value = [...justClosed.reverse(), ...closed.value].slice(0, MAX_CLOSED);
    },
    clearClosed() {
      closed.value = [];
    },
  },
  persist: {
    pick: ['viewMode'],
  },
});
