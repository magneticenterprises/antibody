import type { FullConfig } from '@playwright/test';
import { bootstrap, resetDatabase, seed, startApi, stopApi } from './backend';

/**
 * Fresh database -> bootstrap -> API -> seed through the real HTTP API.
 * Seed data reaches the tests through E2E_SEED (inherited by workers).
 * Returns the teardown that stops the API.
 */
export default async function globalSetup(_config: FullConfig) {
  resetDatabase();
  const token = bootstrap();
  const api = await startApi();
  try {
    const data = await seed(token);
    process.env.E2E_SEED = JSON.stringify(data);
  } catch (e) {
    await stopApi(api);
    throw e;
  }
  return async () => {
    await stopApi(api);
  };
}
