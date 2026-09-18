package io.zeronode.vpn;

import java.util.concurrent.Executor;

final class VpnSessionGate {
    static final class Token {
        final String profileId;
        final String kind;

        Token(String profileId, String kind) {
            this.profileId = profileId;
            this.kind = kind;
        }
    }

    private final Executor executor;
    private Token current;
    private boolean stopped;

    VpnSessionGate(Executor executor) {
        this.executor = executor;
    }

    synchronized Token begin(String profileId, String kind) {
        stopped = false;
        current = new Token(profileId, kind);
        return current;
    }

    synchronized boolean isCurrent(Token token) {
        return token != null && token == current && !stopped;
    }

    synchronized boolean publish(Token token, Runnable callback) {
        if (!isCurrent(token)) return false;
        callback.run();
        return true;
    }

    synchronized void execute(final Token token, final Runnable work) {
        executor.execute(new Runnable() {
            @Override public void run() {
                if (isCurrent(token)) work.run();
            }
        });
    }

    synchronized boolean stopProfile(String profileId, String kind, Runnable cleanup) {
        if (current == null || !current.profileId.equals(profileId) || !current.kind.equals(kind)) return false;
        return stop(cleanup);
    }

    synchronized boolean stop(Runnable cleanup) {
        if (stopped) return false;
        stopped = true;
        current = null;
        executor.execute(cleanup);
        return true;
    }
}
