import { createContext, useContext, type ReactNode } from 'react';
type Auth = { user: string; signOut: () => void };
export const AuthContext = createContext<Auth | null>(null);
function makeAuth(): Auth { return { user: 'u', signOut: () => doSignOut() }; }
function doSignOut(): void {}
export function AuthProvider({ children }: { children: ReactNode }) {
  return <AuthContext.Provider value={makeAuth()}>{children}</AuthContext.Provider>;
}
export function useAuth(): Auth {
  const v = useContext(AuthContext);
  if (!v) throw new Error('no auth');
  return v;
}
