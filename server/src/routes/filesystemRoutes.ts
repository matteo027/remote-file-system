import { Router } from 'express';
import express from 'express';
import { FileSystemController } from '../controllers/filesystemController';
import { Express } from 'express-serve-static-core';
import { AuthenticationController } from '../controllers/authenticationController';

const router = Router();
const filesystemController = new FileSystemController();
const isLoggedIn = (new AuthenticationController).isLoggedIn;

export function setRoutes(app: Express) {
    app.use('/', router);

    router.get('/api/directories/{*path}', isLoggedIn, filesystemController.readdir);
    router.post('/api/directories/{*path}', isLoggedIn, filesystemController.mkdir);
    router.delete('/api/directories/{*path}', isLoggedIn, filesystemController.rmdir);

    router.patch('/api/files/attributes/{*path}', isLoggedIn, filesystemController.setattr);
    router.get('/api/files/attributes/{*path}', isLoggedIn, filesystemController.getattr);

    router.post('/api/files/{*path}', isLoggedIn, filesystemController.create);
    router.put('/api/files/stream/{*path}', isLoggedIn, filesystemController.writeStream);
    router.get('/api/files/stream/{*path}', isLoggedIn, filesystemController.readStream);
    router.put('/api/files/{*path}', isLoggedIn, express.raw({type:'application/octet-stream', limit: '1gb'}), filesystemController.write);
    router.get('/api/files/{*path}', isLoggedIn, filesystemController.read);
    router.delete('/api/files/{*path}', isLoggedIn, filesystemController.unlink);
    router.patch('/api/files/{*path}', isLoggedIn, filesystemController.rename); // rename
}