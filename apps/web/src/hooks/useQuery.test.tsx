import { act, renderHook, waitFor } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { deferred } from '../test/utils';
import { useQuery } from './useQuery';

describe('useQuery response ownership', () => {
  it('ignores a mutation from a previous key without cancelling the current read', async () => {
    const a = deferred<string>();
    const b = deferred<string>();
    const { result, rerender } = renderHook(
      ({ queryKey }) => useQuery(queryKey, () => queryKey === 'a' ? a.promise : b.promise),
      { initialProps: { queryKey: 'a' } },
    );
    const updateA = result.current.setData;
    await act(async () => a.resolve('A'));
    rerender({ queryKey: 'b' });
    expect(result.current.data).toBeUndefined();
    act(() => updateA('Updated A'));
    expect(result.current.data).toBeUndefined();
    expect(result.current.isFetching).toBe(true);
    await act(async () => b.resolve('B'));
    expect(result.current.data).toBe('B');
  });

  it('does not reuse a mutation callback after leaving and returning to its key', async () => {
    const { result, rerender } = renderHook(
      ({ queryKey }) => useQuery(queryKey, async () => queryKey),
      { initialProps: { queryKey: 'a' } },
    );
    await waitFor(() => expect(result.current.data).toBe('a'));
    const updateOriginalA = result.current.setData;
    rerender({ queryKey: 'b' });
    await waitFor(() => expect(result.current.data).toBe('b'));
    rerender({ queryKey: 'a' });
    await waitFor(() => expect(result.current.data).toBe('a'));
    act(() => updateOriginalA('Old A'));
    expect(result.current.data).toBe('a');
  });

  it.each(['success', 'error'])('keeps mutation data when an earlier refresh finishes with %s', async (outcome) => {
    const refresh = deferred<string>();
    let reads = 0;
    const { result } = renderHook(() => useQuery('a', () => ++reads === 1 ? Promise.resolve('A') : refresh.promise));
    await waitFor(() => expect(result.current.data).toBe('A'));
    act(() => result.current.refetch());
    expect(result.current.isFetching).toBe(true);
    act(() => result.current.setData('Updated A'));
    expect(result.current.isFetching).toBe(false);
    await act(async () => {
      if (outcome === 'success') refresh.resolve('Old A');
      else refresh.reject(new Error('Old read failed'));
    });
    expect(result.current.data).toBe('Updated A');
    expect(result.current.error).toBeUndefined();
    expect(result.current.status).toBe('success');
  });
});
