import { messages, type Language } from "./i18n";
import type { ExecutionIdentity } from "./operatorTypes";

export function ExecutionIdentityView({ identity, language }: {
  identity: ExecutionIdentity | null | undefined;
  language: Language;
}) {
  const t = messages[language];
  return <div className="command-audit execution-identity">
    <span>{t.executionIdentity}</span>
    <code title={identity?.account_id}>
      {identity ? `${identity.account_name}${identity.session_id ? ` · ${t.executionSession} ${identity.session_id}` : ""}` : t.executionNotRecorded}
    </code>
  </div>;
}
