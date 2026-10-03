import React from 'react';
import ReactDOM from 'react-dom/client';
import { BrowserRouter } from 'react-router-dom';
import { WebApp } from './WebApp';
import 'antd/dist/reset.css';
import './style.css';
ReactDOM.createRoot(document.getElementById('root')!).render(<React.StrictMode><BrowserRouter><WebApp/></BrowserRouter></React.StrictMode>);
