import { AlertCircle, ArrowLeft, Loader2, ShieldCheck } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useAuth } from './AuthProvider';

export interface AuthCallbackPageProps {
  onSuccess: (redirectPath: string) => void;
  onBackToLogin: () => void;
  search?: string;
}

export function AuthCallbackPage({ onSuccess, onBackToLogin, search }: AuthCallbackPageProps) {
  const { handleCallback } = useAuth();
  const [callbackError, setCallbackError] = useState<string | null>(null);
  const [isProcessing, setIsProcessing] = useState<boolean>(true);
  const startedSearchRef = useRef<string | null>(null);
  const onSuccessRef = useRef(onSuccess);
  onSuccessRef.current = onSuccess;
  const mountedRef = useRef<boolean>(true);

  const effectiveSearch =
    search ?? (typeof window !== 'undefined' && window.location ? window.location.search : '');

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    if (startedSearchRef.current === effectiveSearch) {
      return;
    }
    startedSearchRef.current = effectiveSearch;

    setIsProcessing(true);
    setCallbackError(null);

    void handleCallback(effectiveSearch)
      .then((redirectPath) => {
        if (!mountedRef.current) {
          return;
        }
        setIsProcessing(false);
        onSuccessRef.current(redirectPath);
      })
      .catch((err) => {
        if (!mountedRef.current) {
          return;
        }
        setCallbackError(err instanceof Error ? err.message : 'Authentication callback failed');
        setIsProcessing(false);
      });
  }, [effectiveSearch, handleCallback]);

  return (
    <div className="flex min-h-screen items-center justify-center bg-slate-950 px-4 py-12 text-slate-100">
      <div className="w-full max-w-md rounded-2xl border border-slate-800 bg-slate-900/80 p-8 shadow-2xl">
        {isProcessing && !callbackError ? (
          <div className="flex flex-col items-center gap-4 text-center">
            <div className="flex h-12 w-12 items-center justify-center rounded-full bg-emerald-500/15 text-emerald-400">
              <Loader2 className="h-6 w-6 animate-spin" />
            </div>
            <div className="space-y-1">
              <h1 className="text-lg font-semibold text-white">Completing Sign-In</h1>
              <p className="text-sm text-slate-400">
                Verifying PKCE state and exchanging authorization code...
              </p>
            </div>
          </div>
        ) : callbackError ? (
          <div className="space-y-5">
            <div className="flex items-start gap-3">
              <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-rose-500/15 text-rose-400">
                <AlertCircle className="h-5 w-5" />
              </div>
              <div className="space-y-1">
                <h1 className="text-lg font-semibold text-white">Sign-In Failed</h1>
                <p className="text-sm text-rose-300" role="alert">
                  {callbackError}
                </p>
              </div>
            </div>
            <button
              type="button"
              onClick={onBackToLogin}
              className="inline-flex w-full items-center justify-center gap-2 rounded-lg bg-slate-800 px-4 py-2.5 text-sm font-medium text-slate-100 transition hover:bg-slate-700 focus:outline-none focus:ring-2 focus:ring-emerald-500"
            >
              <ArrowLeft className="h-4 w-4" />
              Back to Sign In
            </button>
          </div>
        ) : (
          <div className="flex flex-col items-center gap-3 text-center">
            <div className="flex h-12 w-12 items-center justify-center rounded-full bg-emerald-500/15 text-emerald-400">
              <ShieldCheck className="h-6 w-6" />
            </div>
            <p className="text-sm text-slate-300">Signed in successfully. Redirecting...</p>
          </div>
        )}
      </div>
    </div>
  );
}
