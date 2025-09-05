import { Router } from 'express';
import express from 'express';
import { FileController } from '../controllers/fileController';
import { ReadWriteController } from '../controllers/RWController';
import { AttributeController } from '../controllers/attrController';
import { Express } from 'express-serve-static-core';
import { AuthenticationController } from '../controllers/authenticationController';

const router = Router();
const fileController = new FileController();
const rwController = new ReadWriteController();
const attrController = new AttributeController();
const isLoggedIn = (new AuthenticationController).isLoggedIn;

export function setRoutes(app: Express) {
    app.use('/', router);

    router.get('/api/entries/:parentIno/:name', isLoggedIn, attrController.lookup);

    router.get('/api/directories/:ino', isLoggedIn, attrController.readdir);
    router.post('/api/directories/:parentIno', isLoggedIn, fileController.mkdir);
    router.delete('/api/directories/:parentIno', isLoggedIn, fileController.rmdir);

    router.patch('/api/files/attributes/{*path}', isLoggedIn, attrController.setattr);
    router.get('/api/files/attributes/:ino', isLoggedIn, attrController.getattr);

    router.post('/api/files/:parentIno', isLoggedIn, fileController.create);
    router.put('/api/files/stream/{*path}', isLoggedIn, rwController.writeStream);
    router.get('/api/files/stream/{*path}', isLoggedIn, rwController.readStream);
    router.put('/api/files/{*path}', isLoggedIn, express.raw({type:'application/octet-stream', limit: '1gb'}), rwController.write);
    router.get('/api/files/{*path}', isLoggedIn, rwController.read);
    router.delete('/api/files/:parentIno', isLoggedIn, fileController.unlink);
    router.patch('/api/files/:oldParentIno/:oldName', isLoggedIn, fileController.rename); // rename
}