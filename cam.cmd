@echo off
setlocal
set "NODE_EXE=%CAM_NODE_EXE%"
if "%NODE_EXE%"=="" set "NODE_EXE=node"
"%NODE_EXE%" "%~dp0bin\cam.js" %*
