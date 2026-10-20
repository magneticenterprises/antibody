import { Route, Routes } from 'react-router';
import { AuthProvider } from './auth/AuthContext';
import { Layout, NotFound, RequireAdmin, RequireAuth } from './components/Layout';
import { ActivityPage } from './routes/activity/ActivityPage';
import { SessionDetailPage } from './routes/activity/SessionDetailPage';
import { KnowledgePage } from './routes/KnowledgePage';
import { LoginPage } from './routes/LoginPage';
import { OverviewPage } from './routes/OverviewPage';
import { RepositoriesPage } from './routes/RepositoriesPage';
import { SettingsPage } from './routes/settings/SettingsPage';
import { SolutionDetailPage } from './routes/SolutionDetailPage';

export function AppRoutes() {
  return (
    <Routes>
      <Route path="/login" element={<LoginPage />} />
      <Route
        element={
          <RequireAuth>
            <Layout />
          </RequireAuth>
        }
      >
        <Route index element={<OverviewPage />} />
        <Route path="repositories" element={<RepositoriesPage />} />
        <Route path="knowledge" element={<KnowledgePage />} />
        <Route path="knowledge/:id" element={<SolutionDetailPage />} />
        <Route path="activity" element={<ActivityPage />} />
        <Route path="activity/:tab" element={<ActivityPage />} />
        <Route path="activity/sessions/:id" element={<SessionDetailPage />} />
        <Route
          path="settings/*"
          element={
            <RequireAdmin what="organization settings">
              <SettingsPage />
            </RequireAdmin>
          }
        />
        <Route path="*" element={<NotFound />} />
      </Route>
    </Routes>
  );
}

export function App() {
  return (
    <AuthProvider>
      <AppRoutes />
    </AuthProvider>
  );
}
