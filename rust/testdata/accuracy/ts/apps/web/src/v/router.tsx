import { createBrowserRouter } from 'react-router-dom';
function RoutedPage() { return <p>{routedText()}</p>; }
function routedText(): string { return 'r'; }
export const router = createBrowserRouter([{ path: '/r', element: <RoutedPage /> }, { path: '/lazy', lazy: () => import('./lazyroute') }]);
