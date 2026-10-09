import { lazy, Suspense } from 'react';
import { BrowserRouter, Routes, Route } from 'react-router-dom';
import { PaperList } from './components/PaperList';
import { AuthProvider } from './ctx/AuthContext';
const About = lazy(() => import('./pages/AboutPage'));
export async function runHeavy(): Promise<number> {
  const m = await import('./pages/heavy');
  return m.heavyCompute();
}
export default function App() {
  return (
    <AuthProvider>
      <BrowserRouter>
        <Suspense fallback={null}>
          <Routes>
            <Route path="/" element={<PaperList />} />
            <Route path="/about" element={<About />} />
          </Routes>
        </Suspense>
      </BrowserRouter>
    </AuthProvider>
  );
}
