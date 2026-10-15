// X42 deinit and extension members. `deinit` has no symbol (the runtime calls it),
// so its body's calls belong to the type, as `init`'s do (C09).
public final class Session {
    private var isOpen = true

    public init() {}

    deinit {
        self.cleanup()                      // X42 deinit body -> Session.cleanup, attributed to Session
    }

    public func close() {
        release()                           // X42 implicit self -> extension member Session.release
    }
}

extension Session {
    func release() {
        cleanup()                           // X42 implicit self inside an extension -> Session.cleanup
    }

    func cleanup() {
        isOpen = false
    }
}

public func retire(_ session: Session) {
    session.close()                         // X42 free function, typed parameter -> Session.close
}
