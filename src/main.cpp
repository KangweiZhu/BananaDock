#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QQuickWindow>
#include <QScreen>
#include <QStandardPaths>


#include "docksurface.h"

static void rawMessageHandler(QtMsgType type, const QMessageLogContext &ctx, const QString &msg)
{
    Q_UNUSED(type)
    fprintf(stderr, "[qt] %s (%s:%d)\n",
            qPrintable(msg),
            ctx.file ? ctx.file : "?",
            ctx.line);
    fflush(stderr);
}

int main(int argc, char *argv[])
{
    qInstallMessageHandler(rawMessageHandler);
    // Since Qt 6.5 no opt-in call is needed: asking LayerShellQt::Window::get()
    // for an as-yet-uncreated window is what selects the layer-shell role.
    QGuiApplication app(argc, argv);
    app.setApplicationName(QStringLiteral("macdock"));
    app.setDesktopFileName(QStringLiteral("macdock"));
    app.setQuitOnLastWindowClosed(false);

    DockSurface surface;

    QQmlApplicationEngine engine;
    engine.rootContext()->setContextProperty(QStringLiteral("DockSurface"), &surface);

    // Resolved here rather than in QML: a QML singleton cannot read context
    // properties, and QtCore's QML module does not expose StandardPaths. Going
    // through QStandardPaths also honours XDG_DATA_HOME.
    engine.rootContext()->setContextProperty(
        QStringLiteral("TrashPath"),
        QStandardPaths::writableLocation(QStandardPaths::GenericDataLocation)
            + QStringLiteral("/Trash/files"));
    engine.loadFromModule("MacDock", "Main");

    if (engine.rootObjects().isEmpty()) {
        return 1;
    }

    auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().constFirst());
    if (!window) {
        qWarning("The root object of Main.qml must be a Window");
        return 1;
    }

    // attach() must run before show(): the layer-shell role can only be set
    // while the surface is being created.
    surface.attach(window);
    window->show();


    return app.exec();
}
