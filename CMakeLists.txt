cmake_minimum_required(VERSION 3.21)

project(macdock VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

find_package(Qt6 6.6 REQUIRED COMPONENTS Core Gui Qml Quick)
find_package(LayerShellQt REQUIRED)

qt_standard_project_setup(REQUIRES 6.6)

qt_add_executable(macdock
    src/main.cpp
    src/docksurface.cpp
    src/docksurface.h
)

# Metrics is a singleton holding every pixel value. This must be declared
# before qt_add_qml_module.
set_source_files_properties(qml/Metrics.qml
    PROPERTIES QT_QML_SINGLETON_TYPE TRUE)

qt_add_qml_module(macdock
    URI MacDock
    VERSION 1.0
    QML_FILES
        qml/Main.qml
        qml/Metrics.qml
        qml/DockPanel.qml
        qml/DockItem.qml
)

target_link_libraries(macdock PRIVATE
    Qt6::Core
    Qt6::Gui
    Qt6::Qml
    Qt6::Quick
    LayerShellQt::Interface
)

install(TARGETS macdock RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})
