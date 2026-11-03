import { useCallback, useEffect, useState } from "react";
import { type Category, type CategoryKind, engine, hasEngine } from "./engine";

/**
 * Filing rules. Unlike the queue, categories are not pushed on a timer —
 * edits are rare and always originate from this view, so a refetch after
 * each mutation is simpler than threading another event through the engine.
 */
export function useCategories() {
  const [items, setItems] = useState<Category[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!hasEngine()) {
      setLoading(false);
      return;
    }
    try {
      setItems(await engine.categories());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const act = useCallback(
    async (fn: () => Promise<unknown>) => {
      setError(null);
      try {
        await fn();
        await refresh();
        return true;
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        return false;
      }
    },
    [refresh],
  );

  const add = useCallback(
    (name: string, kind: CategoryKind, pattern: string, directory: string, priority?: number) =>
      act(() => engine.addCategory(name, kind, pattern, directory, priority)),
    [act],
  );

  const setEnabled = useCallback(
    (id: number, enabled: boolean) => act(() => engine.setCategoryEnabled(id, enabled)),
    [act],
  );

  const remove = useCallback((id: number) => act(() => engine.removeCategory(id)), [act]);

  return { items, loading, error, clearError: () => setError(null), add, setEnabled, remove };
}
