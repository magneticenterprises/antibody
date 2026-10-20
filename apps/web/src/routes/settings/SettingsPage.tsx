import { useParams } from 'react-router';
import { PageHeader, TabNav } from '../../components/common';
import { EmptyState } from '../../components/Status';
import { useDocumentTitle } from '../../hooks/misc';
import { AccessTab } from './AccessTab';
import { AuditSection } from './AuditSection';
import { OrganizationTab } from './OrganizationTab';
import { WorkflowSection } from './WorkflowSection';

const SECTIONS = [
  { key: 'access', to: '/settings/access', label: 'Access' },
  { key: 'organization', to: '/settings/organization', label: 'Organization & data' },
  { key: 'workflow', to: '/settings/workflow', label: 'Workflow improvements' },
  { key: 'audit', to: '/settings/audit', label: 'Audit log' },
];

export function SettingsPage() {
  useDocumentTitle('Settings');
  const rest = useParams()['*'] ?? '';
  const section = rest === '' ? 'access' : rest.split('/')[0] ?? 'access';
  return (
    <>
      <PageHeader title="Settings" lede="Administration for this organization. Sensitive actions are recorded in the audit log." />
      <TabNav label="Settings sections" active={section} items={SECTIONS} />
      {section === 'access' ? <AccessTab /> : null}
      {section === 'organization' ? <OrganizationTab /> : null}
      {section === 'workflow' ? <WorkflowSection /> : null}
      {section === 'audit' ? <AuditSection /> : null}
      {!SECTIONS.some((s) => s.key === section) ? <EmptyState title="Unknown settings section." /> : null}
    </>
  );
}
