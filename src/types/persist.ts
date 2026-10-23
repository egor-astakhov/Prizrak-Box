// @ts-nocheck

// 内存缓存
export const memoryCache: Record<string, string> = {};

// 自定义存储（模拟同步）
export const customStorage = {
    getItem: (key: string): string | null => {
        return memoryCache[key] ?? null;
    },

    setItem: (key: string, value: string): void => {
        memoryCache[key] = value; // 先存入缓存
        if (window["pxStore"]) {
            window["pxStore"].set(key, value)
        }
    }
};

// Persistence options for stores that change many times a second: same
// localStorage key and JSON format as `persist: true`, but the state is
// serialised and written at most once per `delay` ms (and flushed when the page
// goes away). The plugin otherwise JSON.stringifies the WHOLE store on every
// mutation, which for the web store (profile list with embedded logos + up to
// 1000 log lines, a few hundred KB) on every incoming log line kept the
// renderer and the WebView2 storage process busy for as long as the app ran.
//
// The serializer hands the live state object through untouched and the storage
// stringifies it only when it flushes, so it always writes the latest state.
const deferredFlushers: Array<() => void> = [];

// Writes out everything deferredPersist still holds. The native shell tears the
// webview down without a reliable pagehide/beforeunload, so the quit paths call
// this before asking the shell to exit (see wails-shim.ts).
export function flushDeferredPersist(): void {
    for (const flush of deferredFlushers) flush();
}

export function deferredPersist(delay = 2000) {
    const pending: Record<string, any> = {};
    let timer: ReturnType<typeof setTimeout> | null = null;
    const flush = () => {
        if (timer) {
            clearTimeout(timer);
            timer = null;
        }
        for (const key of Object.keys(pending)) {
            try {
                localStorage.setItem(key, JSON.stringify(pending[key]));
            } catch {
                /* quota / unavailable: keep the in-memory state */
            }
            delete pending[key];
        }
    };
    deferredFlushers.push(flush);
    window.addEventListener('pagehide', flush);
    window.addEventListener('beforeunload', flush);
    document.addEventListener('visibilitychange', () => {
        if (document.hidden) flush();
    });
    return {
        serializer: {
            serialize: (state: any) => state,
            deserialize: (raw: string) => JSON.parse(raw),
        },
        storage: {
            getItem: (key: string): string | null =>
                key in pending ? JSON.stringify(pending[key]) : localStorage.getItem(key),
            setItem: (key: string, state: any): void => {
                pending[key] = state;
                if (!timer) timer = setTimeout(flush, delay);
            },
        },
    };
}

// 持久化配置
export const defaultPersist = {
    enabled: true,
    storage: customStorage
};
